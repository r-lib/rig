//! `rig pkg tree`: the dependencies of a package, as a tree.
//!
//! The same transitive closure [`super::deps`] prints as a flat table, but laid
//! out so the shape of the graph is visible: each package's dependencies are
//! shown once, under its first occurrence, and later occurrences are leaves
//! marked `(*)`. That keeps the output readable — a popular package like `rlang`
//! is in most of the closure — and makes dependency cycles terminate on their
//! own.

use std::collections::HashSet;
use std::env;
use std::error::Error;
use std::fmt::Write;
use std::io::IsTerminal;

use clap::ArgMatches;

use super::deps::{
    newest_version, requirements, root_package, type_rank, version_cell_for, wanted_dep, Newest,
};
use crate::dcf::{DepVersionSpec, RDepType, RPackageVersion, DEP_TYPES_SOFT};
use crate::repos::DbSourcePackageLoader;
use crate::solver::{is_base_package, PackageVersionLoader};

pub fn sc_pkg_tree(
    args: &ArgMatches,
    pkgargs: &ArgMatches,
    mainargs: &ArgMatches,
) -> Result<(), Box<dyn Error>> {
    let package: String = args.get_one::<String>("package").unwrap().to_string();
    let ver = if args.contains_id("version") {
        args.get_one::<String>("version").unwrap().to_string()
    } else {
        "latest".to_string()
    };
    let dev = args.get_flag("dev");
    let no_base = args.get_flag("no-base");
    let json = args.get_flag("json") || pkgargs.get_flag("json") || mainargs.get_flag("json");

    let loader = DbSourcePackageLoader::new()?;
    let tree = dep_tree(&loader, &package, &ver, dev, no_base)?;

    print_tree(&tree, json)
}

/// `rig proj tree`: the dependency tree of a project's manifest.
///
/// The root is the project itself, which is not a package in the repositories,
/// so the walk starts from the dependency list `crate::proj::proj_read_deps`
/// read from the manifest. That list has already had the soft dependencies
/// dropped unless `--dev`, so the walk takes it as it is — the same `dev = true`
/// `rig proj deps --recursive` passes to [`super::deps::walk_deps`].
pub(crate) fn proj_tree(
    root_name: &str,
    root_version: &RPackageVersion,
    root_deps: &[DepVersionSpec],
    no_base: bool,
    json: bool,
) -> Result<(), Box<dyn Error>> {
    let loader = DbSourcePackageLoader::new()?;
    let tree = tree_from_deps(
        &loader,
        root_name,
        Some(root_version.clone()),
        root_deps,
        true,
        no_base,
    );

    print_tree(&tree, json)
}

/// Print a tree, as one nested JSON object with `--json`, otherwise as the
/// colored tree, with the color left out when stdout is not a terminal.
fn print_tree(tree: &DepTree, json: bool) -> Result<(), Box<dyn Error>> {
    if json {
        print_tree_json(tree)?;
    } else {
        let color = std::io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none();
        print!("{}", render_tree(tree, color));
    }

    Ok(())
}

// ------------------------------------------------------------------------
// Building the tree

/// One package in the tree: how the parent needs it, what the repositories
/// currently offer, and what it needs in turn.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TreeNode {
    name: String,
    /// Newest version in the database. `None` for R and the base packages,
    /// which ship with R, and for a package the database does not know about.
    version: Option<RPackageVersion>,
    /// Dependency type(s) the parent needs this package with, e.g. `Imports`.
    /// Empty for the root, which nothing needs.
    types: Vec<RDepType>,
    /// Version requirement(s) of the parent, e.g. `>= 1.0.2`. Empty for the
    /// root.
    requires: Vec<String>,
    children: Vec<TreeNode>,
    /// Whether this package's dependencies are shown under an earlier
    /// occurrence instead of here.
    repeat: bool,
}

/// A whole dependency tree, with the counts the header line reports.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DepTree {
    root: TreeNode,
    /// Number of distinct packages in the tree, not counting the root itself.
    total: usize,
}

/// The dependency tree of one version of a package.
///
/// With `dev`, the soft dependencies (`Suggests`, `Enhances`) of the queried
/// package are part of the tree, but only hard dependencies are followed below
/// that, i.e. the walk does not visit the `Suggests` of a `Suggests`. With
/// `no_base`, R and the base packages are left out altogether.
///
/// Like [`super::deps`], the tree is taken over the newest version of each
/// package, so a version requirement that would force an older version — with
/// different dependencies — is not honored. `rig proj solve` is what a full,
/// version-consistent resolution is for.
fn dep_tree(
    loader: &dyn PackageVersionLoader,
    package: &str,
    ver: &str,
    dev: bool,
    no_base: bool,
) -> Result<DepTree, Box<dyn Error>> {
    let root = root_package(loader, package, ver)?;
    Ok(tree_from_deps(
        loader,
        package,
        Some(root.version),
        &root.dependencies.dependencies,
        dev,
        no_base,
    ))
}

/// The dependency tree below a list of direct dependencies, e.g. the ones a
/// package version declares or the ones a project's `DESCRIPTION` does.
///
/// `root_name` and `root_version` are only the label of the header line; the
/// root does not have to be a package the repositories know about, which is what
/// lets a project be one. See [`dep_tree`] for what the walk does and does not
/// follow.
fn tree_from_deps(
    loader: &dyn PackageVersionLoader,
    root_name: &str,
    root_version: Option<RPackageVersion>,
    root_deps: &[DepVersionSpec],
    dev: bool,
    no_base: bool,
) -> DepTree {
    let mut newest = Newest::new(loader);

    // The root's subtree *is* the tree, so it counts as expanded from the
    // start: a cycle that comes back to it collapses into a `(*)` leaf.
    let mut expanded: HashSet<String> = HashSet::new();
    expanded.insert(root_name.to_string());
    let mut seen: HashSet<String> = HashSet::new();

    let children = children_of(
        root_deps,
        &mut newest,
        &mut expanded,
        &mut seen,
        dev,
        no_base,
    );

    let mut tree = DepTree {
        root: TreeNode {
            name: root_name.to_string(),
            version: root_version,
            types: vec![],
            requires: vec![],
            children,
            repeat: false,
        },
        total: seen.len(),
    };
    prune_empty_repeats(&mut tree.root);

    tree
}

/// Turn the dependency list of one package into the nodes below it, expanding
/// each package the first time it is seen.
///
/// The nodes are sorted the way they are printed *before* the walk descends into
/// them, so the first occurrence of a package in the output is the one that has
/// its subtree expanded, and a `(*)` always points at a line above it.
fn children_of(
    deps: &[DepVersionSpec],
    newest: &mut Newest,
    expanded: &mut HashSet<String>,
    seen: &mut HashSet<String>,
    dev: bool,
    no_base: bool,
) -> Vec<TreeNode> {
    let mut wanted: Vec<&DepVersionSpec> = deps
        .iter()
        .filter(|dep| wanted_dep(dep, dev))
        .filter(|dep| !no_base || !is_base_package(&dep.name))
        .collect();

    // R first, then group by dependency type, in the order R lists the fields
    // in, and sort by name within a type — the same order `rig pkg deps` uses.
    wanted.sort_by(|a, b| {
        r_first(&a.name).cmp(&r_first(&b.name)).then_with(|| {
            type_rank(&a.types)
                .cmp(&type_rank(&b.types))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        })
    });

    let mut nodes: Vec<TreeNode> = vec![];
    for dep in wanted {
        seen.insert(dep.name.clone());
        let mut node = TreeNode {
            name: dep.name.clone(),
            version: newest_version(newest, &dep.name),
            types: dep.types.clone(),
            requires: requirements(dep),
            children: vec![],
            repeat: false,
        };

        if is_base_package(&dep.name) {
            // R and the base packages ship with R, so they are always leaves.
            nodes.push(node);
            continue;
        }
        if !expanded.insert(dep.name.clone()) {
            node.repeat = true;
            nodes.push(node);
            continue;
        }

        // `Newest::get` borrows the memo table that the recursive call needs
        // mutably, so copy the dependency list out before descending. A package
        // that is not in the database, or that we cannot read, is not fatal: we
        // just cannot say what it needs.
        let deps = match newest.get(&dep.name) {
            Some(pkg) => pkg.dependencies.dependencies.clone(),
            None => vec![],
        };
        node.children = children_of(&deps, newest, expanded, seen, false, no_base);
        nodes.push(node);
    }

    nodes
}

/// R is the dependency everything else is relative to, so it goes first.
fn r_first(name: &str) -> usize {
    if name == "R" {
        0
    } else {
        1
    }
}

/// Drop the `(*)` marker from repeats of a package that has no dependencies:
/// the marker means "shown above", and there is nothing to show.
fn prune_empty_repeats(root: &mut TreeNode) {
    let mut expandable: HashSet<String> = HashSet::new();
    collect_expandable(root, &mut expandable);
    clear_repeats(root, &expandable);
}

fn collect_expandable(node: &TreeNode, expandable: &mut HashSet<String>) {
    if !node.children.is_empty() {
        expandable.insert(node.name.clone());
    }
    for child in node.children.iter() {
        collect_expandable(child, expandable);
    }
}

fn clear_repeats(node: &mut TreeNode, expandable: &HashSet<String>) {
    if node.repeat && !expandable.contains(&node.name) {
        node.repeat = false;
    }
    for child in node.children.iter_mut() {
        clear_repeats(child, expandable);
    }
}

// ------------------------------------------------------------------------
// Output

/// Render the tree the way it is printed on a terminal: a colored header line
/// naming the package version, how many direct dependencies it has and how many
/// packages there are altogether, then the tree itself.
fn render_tree(tree: &DepTree, color: bool) -> String {
    use owo_colors::OwoColorize;

    let root = &tree.root;
    let version = root
        .version
        .as_ref()
        .map(|v| v.to_string())
        .unwrap_or_default();
    let tag = format!("{} direct, {} total", root.children.len(), tree.total);

    let mut out = String::new();
    if color {
        let _ = writeln!(
            out,
            "{} {} — {}",
            root.name.cyan().bold(),
            version.bold(),
            tag.dimmed()
        );
    } else {
        let _ = writeln!(out, "{} {} — {}", root.name, version, tag);
    }
    // No blank line between the header and the tree: the header *is* the root
    // node, so the `├──` glyphs below have to connect to it.
    render_children(&root.children, "", color, &mut out);
    out
}

/// Render the children of a node, each on its own line, with the box-drawing
/// prefix that connects it to its parent.
///
/// The soft dependencies go into their own `[Suggests]` / `[Enhances]` sections,
/// the way `cargo tree` sets `[dev-dependencies]` apart, each section numbering
/// its own lines. In practice that only happens under the queried package, as
/// `--dev` does not apply below it.
fn render_children(children: &[TreeNode], prefix: &str, color: bool, out: &mut String) {
    use owo_colors::OwoColorize;

    for (section, group) in sections(children) {
        if let Some(section) = section {
            if color {
                // Not dimmed like the glyphs: a section heading is a divider
                // between two parts of the listing and has to be easy to spot.
                let _ = writeln!(
                    out,
                    "{}{}",
                    prefix.dimmed(),
                    format!("[{}]", section).magenta()
                );
            } else {
                let _ = writeln!(out, "{}[{}]", prefix, section);
            }
        }
        for (i, child) in group.iter().enumerate() {
            let last = i + 1 == group.len();
            let (connector, below) = if last {
                ("└── ", "    ")
            } else {
                ("├── ", "│   ")
            };
            let glyphs = format!("{}{}", prefix, connector);
            if color {
                let _ = writeln!(out, "{}{}", glyphs.dimmed(), node_label(child, color));
            } else {
                let _ = writeln!(out, "{}{}", glyphs, node_label(child, color));
            }
            render_children(&child.children, &format!("{}{}", prefix, below), color, out);
        }
    }
}

/// Split a node's children into the sections they are printed in: first the
/// hard dependencies, unlabelled, then one section per soft dependency type.
///
/// The children are already sorted by [`type_rank`], so each section is a
/// contiguous run and this only has to find the boundaries.
fn sections(children: &[TreeNode]) -> Vec<(Option<RDepType>, &[TreeNode])> {
    let mut out: Vec<(Option<RDepType>, &[TreeNode])> = vec![];
    let mut start = 0;
    for i in 0..children.len() {
        if i > 0 && section_of(&children[i]) != section_of(&children[i - 1]) {
            out.push((section_of(&children[start]), &children[start..i]));
            start = i;
        }
    }
    if start < children.len() {
        out.push((section_of(&children[start]), &children[start..]));
    }
    out
}

/// The section a dependency is printed in: `None` for a hard dependency, which
/// is the unlabelled default, otherwise the soft type it is needed with. A
/// package that is both a hard and a soft dependency is a hard one.
fn section_of(node: &TreeNode) -> Option<RDepType> {
    if !node.types.iter().all(|t| DEP_TYPES_SOFT.contains(t)) {
        return None;
    }
    RDepType::all()
        .iter()
        .find(|t| node.types.contains(t))
        .cloned()
}

/// One line of the tree: the package, the version the repositories offer, the
/// version requirement it is needed with, `[D]` / `[L]` if it is more than a
/// plain import, and `(*)` if its own dependencies are shown further up.
fn node_label(node: &TreeNode, color: bool) -> String {
    use owo_colors::OwoColorize;

    let mut label = if color {
        node.name.bold().to_string()
    } else {
        node.name.clone()
    };

    let mut dim = |text: String| {
        label.push(' ');
        if color {
            label.push_str(&text.dimmed().to_string());
        } else {
            label.push_str(&text);
        }
    };

    // R and the base packages have no version of their own, so we say nothing,
    // rather than printing the `-` the `rig pkg deps` tables use.
    if node.version.is_some() || !is_base_package(&node.name) {
        dim(version_cell_for(&node.name, node.version.as_ref()));
    }
    if !node.requires.is_empty() {
        dim(format!("({})", node.requires.join(", ")));
    }
    if let Some(marks) = type_marks(&node.types) {
        dim(marks);
    }
    if node.repeat {
        dim("(*)".to_string());
    }

    label
}

/// How a dependency is needed, when that is more than a plain `Imports`:
/// `[D]` for `Depends`, i.e. the package is attached and not merely loaded, and
/// `[L]` for `LinkingTo`, i.e. it is compiled against. A package that is both is
/// `[DL]`.
///
/// `Imports` is the common case and stays unmarked; the soft types are not
/// marked either, they have a section of their own.
fn type_marks(types: &[RDepType]) -> Option<String> {
    let mut marks = String::new();
    if types.contains(&RDepType::Depends) {
        marks.push('D');
    }
    if types.contains(&RDepType::LinkingTo) {
        marks.push('L');
    }
    if marks.is_empty() {
        None
    } else {
        Some(format!("[{}]", marks))
    }
}

/// Print the tree as a single nested JSON object. A `repeat` node always has an
/// empty `dependencies` array; its dependencies are under its first occurrence.
fn print_tree_json(tree: &DepTree) -> Result<(), Box<dyn Error>> {
    #[derive(serde::Serialize)]
    struct Node<'a> {
        package: &'a str,
        version: Option<String>,
        types: Vec<String>,
        requires: &'a [String],
        repeat: bool,
        dependencies: Vec<Node<'a>>,
    }

    #[derive(serde::Serialize)]
    struct Root<'a> {
        package: &'a str,
        version: Option<String>,
        direct: usize,
        total: usize,
        dependencies: Vec<Node<'a>>,
    }

    fn nodes(children: &[TreeNode]) -> Vec<Node<'_>> {
        children
            .iter()
            .map(|child| Node {
                package: &child.name,
                version: child.version.as_ref().map(|v| v.to_string()),
                types: child.types.iter().map(|t| t.to_string()).collect(),
                requires: &child.requires,
                repeat: child.repeat,
                dependencies: nodes(&child.children),
            })
            .collect()
    }

    let root = &tree.root;
    let out = Root {
        package: &root.name,
        version: root.version.as_ref().map(|v| v.to_string()),
        direct: root.children.len(),
        total: tree.total,
        dependencies: nodes(&root.children),
    };

    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pkg::stub::{stub_deps, Stub};

    /// The tree as `(indent, label)` pairs, so a test can assert on the shape
    /// without the box-drawing noise.
    fn shape(tree: &DepTree) -> Vec<(usize, String)> {
        fn walk(nodes: &[TreeNode], depth: usize, out: &mut Vec<(usize, String)>) {
            for node in nodes {
                out.push((depth, node_label(node, false)));
                walk(&node.children, depth + 1, out);
            }
        }
        let mut out = vec![];
        walk(&tree.root.children, 0, &mut out);
        out
    }

    /// The tree as the lines `render_tree` prints, without the header.
    fn lines(tree: &DepTree) -> Vec<String> {
        render_tree(tree, false)
            .lines()
            .skip(1)
            .map(|l| l.to_string())
            .collect()
    }

    // ---------------------------------------------------------------------
    // Building the tree

    #[test]
    fn the_tree_nests_the_whole_closure() {
        let stub = Stub {
            packages: vec![
                ("a", "1.0.0", "Imports: b, c"),
                ("b", "1.1.0", "Imports: d"),
                ("c", "1.2.0", "Imports: e"),
                ("d", "1.3.0", ""),
                ("e", "1.4.0", ""),
            ],
        };

        let tree = dep_tree(&stub, "a", "latest", false, false).unwrap();

        assert_eq!(tree.root.name, "a");
        assert_eq!(tree.root.version.as_ref().unwrap().original, "1.0.0");
        assert_eq!(tree.root.children.len(), 2);
        assert_eq!(tree.total, 4);
        assert_eq!(
            shape(&tree),
            vec![
                (0, "b 1.1.0".to_string()),
                (1, "d 1.3.0".to_string()),
                (0, "c 1.2.0".to_string()),
                (1, "e 1.4.0".to_string()),
            ]
        );
    }

    #[test]
    fn children_are_r_first_then_by_type_then_by_name() {
        let stub = Stub {
            packages: vec![
                (
                    "a",
                    "1.0.0",
                    "Depends: R (>= 3.5.0); Imports: zoo, mid (>= 2.0.0); \
                     LinkingTo: cpp11",
                ),
                ("zoo", "1.8.14", ""),
                ("mid", "2.1.0", ""),
                ("cpp11", "0.5.2", ""),
            ],
        };

        let tree = dep_tree(&stub, "a", "latest", false, false).unwrap();
        assert_eq!(
            shape(&tree),
            vec![
                (0, "R (>= 3.5.0) [D]".to_string()),
                (0, "mid 2.1.0 (>= 2.0.0)".to_string()),
                (0, "zoo 1.8.14".to_string()),
                (0, "cpp11 0.5.2 [L]".to_string()),
            ]
        );
    }

    #[test]
    fn a_diamond_is_expanded_once_and_repeated_with_a_marker() {
        let stub = Stub {
            packages: vec![
                ("a", "1.0.0", "Imports: b, c"),
                ("b", "1.0.0", "Imports: d (>= 2.0.0)"),
                ("c", "1.0.0", "Imports: d"),
                ("d", "2.1.0", "Imports: e"),
                ("e", "1.0.0", ""),
            ],
        };

        let tree = dep_tree(&stub, "a", "latest", false, false).unwrap();
        assert_eq!(tree.total, 4);
        assert_eq!(
            shape(&tree),
            vec![
                (0, "b 1.0.0".to_string()),
                (1, "d 2.1.0 (>= 2.0.0)".to_string()),
                (2, "e 1.0.0".to_string()),
                (0, "c 1.0.0".to_string()),
                // Same package, its subtree is above.
                (1, "d 2.1.0 (*)".to_string()),
            ]
        );
    }

    #[test]
    fn a_repeat_of_a_leaf_gets_no_marker() {
        let stub = Stub {
            packages: vec![
                ("a", "1.0.0", "Imports: b, c"),
                ("b", "1.0.0", "Imports: leaf"),
                ("c", "1.0.0", "Imports: leaf"),
                ("leaf", "1.0.0", ""),
            ],
        };

        let tree = dep_tree(&stub, "a", "latest", false, false).unwrap();
        assert_eq!(
            shape(&tree),
            vec![
                (0, "b 1.0.0".to_string()),
                (1, "leaf 1.0.0".to_string()),
                (0, "c 1.0.0".to_string()),
                (1, "leaf 1.0.0".to_string()),
            ]
        );
    }

    #[test]
    fn a_cycle_terminates() {
        let stub = Stub {
            packages: vec![
                ("a", "1.0.0", "Imports: b"),
                ("b", "1.0.0", "Imports: c"),
                ("c", "1.0.0", "Imports: b, a"),
            ],
        };

        let tree = dep_tree(&stub, "a", "latest", false, false).unwrap();
        // The root is part of the closure here, so it is counted.
        assert_eq!(tree.total, 3);
        assert_eq!(
            shape(&tree),
            vec![
                (0, "b 1.0.0".to_string()),
                (1, "c 1.0.0".to_string()),
                (2, "a 1.0.0 (*)".to_string()),
                (2, "b 1.0.0 (*)".to_string()),
            ]
        );
    }

    #[test]
    fn a_self_dependency_terminates() {
        let stub = Stub {
            packages: vec![("a", "1.0.0", "Imports: a, b"), ("b", "1.0.0", "")],
        };

        let tree = dep_tree(&stub, "a", "latest", false, false).unwrap();
        assert_eq!(
            shape(&tree),
            vec![(0, "a 1.0.0 (*)".to_string()), (0, "b 1.0.0".to_string())]
        );
    }

    #[test]
    fn base_packages_are_leaves_and_no_base_drops_them() {
        let stub = Stub {
            packages: vec![
                ("a", "1.0.0", "Depends: R (>= 3.5.0); Imports: b, stats"),
                ("b", "1.0.0", "Imports: utils"),
                // Never consulted: `stats` is a base package.
                ("stats", "0.0.1", "Imports: neverseen"),
                ("neverseen", "1.0.0", ""),
            ],
        };

        let tree = dep_tree(&stub, "a", "latest", false, false).unwrap();
        assert_eq!(tree.total, 4);
        assert_eq!(
            shape(&tree),
            vec![
                (0, "R (>= 3.5.0) [D]".to_string()),
                (0, "b 1.0.0".to_string()),
                (1, "utils".to_string()),
                (0, "stats".to_string()),
            ]
        );

        let tree = dep_tree(&stub, "a", "latest", false, true).unwrap();
        assert_eq!(tree.total, 1);
        assert_eq!(shape(&tree), vec![(0, "b 1.0.0".to_string())]);
    }

    #[test]
    fn dev_only_applies_to_the_queried_package() {
        let stub = Stub {
            packages: vec![
                ("a", "1.0.0", "Imports: b; Suggests: t"),
                ("b", "1.0.0", ""),
                // `t`'s own Suggests is not followed, its Imports is.
                ("t", "1.0.0", "Imports: ti; Suggests: tt"),
                ("ti", "1.0.0", ""),
                ("tt", "1.0.0", ""),
            ],
        };

        let tree = dep_tree(&stub, "a", "latest", false, false).unwrap();
        assert_eq!(shape(&tree), vec![(0, "b 1.0.0".to_string())]);

        let tree = dep_tree(&stub, "a", "latest", true, false).unwrap();
        assert_eq!(tree.total, 3);
        assert_eq!(
            shape(&tree),
            vec![
                (0, "b 1.0.0".to_string()),
                (0, "t 1.0.0".to_string()),
                (1, "ti 1.0.0".to_string()),
            ]
        );
    }

    #[test]
    fn a_dep_missing_from_the_database_is_a_leaf() {
        let stub = Stub {
            packages: vec![("a", "1.0.0", "Imports: gone, b"), ("b", "1.0.0", "")],
        };

        let tree = dep_tree(&stub, "a", "latest", false, false).unwrap();
        assert_eq!(
            shape(&tree),
            vec![(0, "b 1.0.0".to_string()), (0, "gone ?".to_string())]
        );
    }

    /// The tree and `rig pkg deps --recursive` must agree on what the closure
    /// is: they follow the same edges, so the tree's distinct non-root package
    /// names are exactly the rows of the flat listing.
    #[test]
    fn the_tree_covers_the_same_closure_as_the_flat_listing() {
        let stub = Stub {
            packages: vec![
                (
                    "a",
                    "1.0.0",
                    "Depends: R (>= 3.5.0); Imports: b, c, stats; Suggests: t",
                ),
                ("b", "1.0.0", "Imports: d; LinkingTo: cpp11"),
                ("c", "1.0.0", "Imports: d, utils"),
                ("d", "1.0.0", "Imports: a, gone"),
                ("cpp11", "1.0.0", ""),
                ("t", "1.0.0", "Imports: ti"),
                ("ti", "1.0.0", ""),
            ],
        };

        for dev in [false, true] {
            let tree = dep_tree(&stub, "a", "latest", dev, false).unwrap();
            let mut in_tree: Vec<String> = names(&tree.root).into_iter().collect();
            in_tree.sort();

            let mut flat = super::super::deps::recursive_dep_names(&stub, "a", "latest", dev)
                .unwrap()
                .to_vec();
            flat.sort();

            assert_eq!(in_tree, flat, "dev = {}", dev);
            assert_eq!(tree.total, flat.len(), "dev = {}", dev);
        }
    }

    /// The distinct package names in the tree, not counting the root node.
    fn names(root: &TreeNode) -> HashSet<String> {
        fn walk(nodes: &[TreeNode], out: &mut HashSet<String>) {
            for node in nodes {
                out.insert(node.name.clone());
                walk(&node.children, out);
            }
        }
        let mut out = HashSet::new();
        walk(&root.children, &mut out);
        out
    }

    #[test]
    fn version_selects_the_deps_of_that_version() {
        let stub = Stub {
            packages: vec![
                ("a", "1.0.0", "Imports: old"),
                ("a", "2.0.0", "Imports: new"),
                ("old", "1.0.0", ""),
                ("new", "1.0.0", ""),
            ],
        };

        let tree = dep_tree(&stub, "a", "1.0.0", false, false).unwrap();
        assert_eq!(tree.root.version.as_ref().unwrap().original, "1.0.0");
        assert_eq!(shape(&tree), vec![(0, "old 1.0.0".to_string())]);
    }

    #[test]
    fn unknown_package_and_version_are_errors() {
        let stub = Stub {
            packages: vec![("a", "1.0.0", "")],
        };

        let err = dep_tree(&stub, "nosuchpkg", "latest", false, false).unwrap_err();
        assert!(err.to_string().contains("Could not find package"));

        let err = dep_tree(&stub, "a", "9.9.9", false, false).unwrap_err();
        assert!(err.to_string().contains("Could not find version"));
    }

    // ---------------------------------------------------------------------
    // A project as the root, i.e. `rig proj tree`

    /// The tree of a project, the way `rig proj tree` builds it: the root is
    /// not a package in the database, only its dependencies are, and the soft
    /// dependencies were already filtered by `proj_read_deps`, so the walk runs
    /// with `dev = true`.
    fn project_tree(stub: &Stub, deps: &str, no_base: bool) -> DepTree {
        let root = stub_deps(deps);
        tree_from_deps(
            stub,
            "myproj",
            Some(RPackageVersion::from_str("0.1.0").unwrap()),
            &root.dependencies,
            true,
            no_base,
        )
    }

    #[test]
    fn a_project_root_needs_no_package_in_the_database() {
        let stub = Stub {
            packages: vec![
                ("b", "1.0.0", "Depends: R (>= 4.1.0); Imports: c, utils"),
                ("c", "2.0.0", "Imports: d"),
                ("d", "3.0.0", "Imports: e"),
                ("e", "4.0.0", ""),
                ("t", "1.0.0", "Imports: d"),
            ],
        };

        let tree = project_tree(&stub, "Imports: b (>= 0.9.0); Suggests: t", false);

        assert_eq!(tree.root.name, "myproj");
        assert_eq!(tree.root.version.as_ref().unwrap().original, "0.1.0");
        assert_eq!(tree.total, 7);
        assert_eq!(
            render_tree(&tree, false).lines().next().unwrap(),
            "myproj 0.1.0 — 2 direct, 7 total"
        );
        assert_eq!(
            lines(&tree),
            vec![
                "└── b 1.0.0 (>= 0.9.0)",
                "    ├── R (>= 4.1.0) [D]",
                "    ├── c 2.0.0",
                "    │   └── d 3.0.0",
                "    │       └── e 4.0.0",
                "    └── utils",
                // The soft dependency the manifest kept gets its own section,
                // and its own Suggests is not followed below it.
                "[Suggests]",
                "└── t 1.0.0",
                "    └── d 3.0.0 (*)",
            ]
        );
    }

    /// The project tree and `rig proj deps --recursive` must agree on what the
    /// closure is: they read the same manifest and follow the same edges, so
    /// the tree's distinct non-root package names are exactly the rows of the
    /// flat listing.
    #[test]
    fn the_project_tree_covers_the_same_closure_as_the_flat_listing() {
        let stub = Stub {
            packages: vec![
                ("b", "1.0.0", "Depends: R (>= 4.1.0); Imports: c, stats"),
                ("c", "2.0.0", "Imports: d, gone"),
                ("d", "3.0.0", "Imports: b"),
                ("t", "1.0.0", "Imports: d; Suggests: tt"),
                ("tt", "1.0.0", ""),
            ],
        };
        let deps = "Imports: b; LinkingTo: cpp11; Suggests: t";

        let tree = project_tree(&stub, deps, false);
        let mut in_tree: Vec<String> = names(&tree.root).into_iter().collect();
        in_tree.sort();

        let root = stub_deps(deps);
        let (rows, num_direct) =
            super::super::deps::walk_deps(&stub, "myproj", &root.dependencies, true);
        let mut flat = super::super::deps::dep_row_names(&rows);
        flat.sort();

        assert_eq!(in_tree, flat);
        assert_eq!(tree.total, flat.len());
        assert_eq!(tree.root.children.len(), num_direct);
    }

    #[test]
    fn no_base_drops_r_and_the_base_packages_from_a_project_tree() {
        let stub = Stub {
            packages: vec![("b", "1.0.0", "Imports: utils, c"), ("c", "2.0.0", "")],
        };

        let deps = "Depends: R (>= 4.1.0); Imports: b, stats";
        let tree = project_tree(&stub, deps, false);
        assert_eq!(tree.total, 5);
        assert_eq!(
            shape(&tree),
            vec![
                (0, "R (>= 4.1.0) [D]".to_string()),
                (0, "b 1.0.0".to_string()),
                (1, "c 2.0.0".to_string()),
                (1, "utils".to_string()),
                (0, "stats".to_string()),
            ]
        );

        let tree = project_tree(&stub, deps, true);
        assert_eq!(tree.total, 2);
        assert_eq!(
            shape(&tree),
            vec![(0, "b 1.0.0".to_string()), (1, "c 2.0.0".to_string())]
        );
    }

    /// A project whose manifest names a package that is also in the
    /// repositories, and that its own closure depends on: the root is expanded
    /// from the start, so the cycle back to it is a `(*)` leaf, not a loop.
    #[test]
    fn a_cycle_back_to_the_project_terminates() {
        let stub = Stub {
            packages: vec![
                ("myproj", "0.0.1", "Imports: neverseen"),
                ("b", "1.0.0", "Imports: myproj"),
                ("neverseen", "1.0.0", ""),
            ],
        };

        let tree = project_tree(&stub, "Imports: b", false);
        assert_eq!(
            shape(&tree),
            vec![
                (0, "b 1.0.0".to_string()),
                // The database's `myproj` is not expanded: the root is.
                (1, "myproj 0.0.1 (*)".to_string()),
            ]
        );
    }

    // ---------------------------------------------------------------------
    // Formatting

    #[test]
    fn the_glyphs_connect_each_node_to_its_parent() {
        let stub = Stub {
            packages: vec![
                ("a", "1.0.0", "Imports: b, c"),
                ("b", "1.0.0", "Imports: d, e"),
                ("c", "1.0.0", ""),
                ("d", "1.0.0", "Imports: f"),
                ("e", "1.0.0", ""),
                ("f", "1.0.0", ""),
            ],
        };

        let tree = dep_tree(&stub, "a", "latest", false, false).unwrap();
        assert_eq!(
            lines(&tree),
            vec![
                "├── b 1.0.0",
                "│   ├── d 1.0.0",
                "│   │   └── f 1.0.0",
                "│   └── e 1.0.0",
                "└── c 1.0.0",
            ]
        );
    }

    #[test]
    fn the_header_counts_direct_and_total() {
        let stub = Stub {
            packages: vec![
                ("a", "1.0.0", "Imports: b, c"),
                ("b", "1.0.0", "Imports: d"),
                ("c", "1.0.0", ""),
                ("d", "1.0.0", ""),
            ],
        };

        let tree = dep_tree(&stub, "a", "latest", false, false).unwrap();
        let out = render_tree(&tree, false);
        assert_eq!(out.lines().next().unwrap(), "a 1.0.0 — 2 direct, 3 total");
    }

    #[test]
    fn a_package_with_no_dependencies_is_just_the_header() {
        let stub = Stub {
            packages: vec![("a", "1.0.0", "")],
        };

        let tree = dep_tree(&stub, "a", "latest", false, false).unwrap();
        assert_eq!(render_tree(&tree, false), "a 1.0.0 — 0 direct, 0 total\n");
    }

    #[test]
    fn the_soft_deps_go_into_their_own_sections() {
        let stub = Stub {
            packages: vec![
                (
                    "a",
                    "1.0.0",
                    "Depends: dep; Imports: imp; LinkingTo: cpp11; \
                     Suggests: s1, s2; Enhances: e",
                ),
                ("dep", "1.0.0", ""),
                ("imp", "1.0.0", ""),
                ("cpp11", "1.0.0", ""),
                ("s1", "1.0.0", "Imports: si"),
                ("s2", "1.0.0", ""),
                ("si", "1.0.0", ""),
                ("e", "1.0.0", ""),
            ],
        };

        let tree = dep_tree(&stub, "a", "latest", true, false).unwrap();
        assert_eq!(
            lines(&tree),
            vec![
                // The hard dependencies are the unlabelled default section, and
                // `Depends` / `LinkingTo` are marked on the line.
                "├── dep 1.0.0 [D]",
                "├── imp 1.0.0",
                "└── cpp11 1.0.0 [L]",
                // Each soft type gets a section, numbering its own lines.
                "[Suggests]",
                "├── s1 1.0.0",
                "│   └── si 1.0.0",
                "└── s2 1.0.0",
                "[Enhances]",
                "└── e 1.0.0",
            ]
        );

        // Without `--dev` there are no soft deps, so no sections either.
        let tree = dep_tree(&stub, "a", "latest", false, false).unwrap();
        assert_eq!(
            lines(&tree),
            vec!["├── dep 1.0.0 [D]", "├── imp 1.0.0", "└── cpp11 1.0.0 [L]"]
        );
    }

    #[test]
    fn a_dep_that_is_both_hard_and_soft_stays_in_the_default_section() {
        let stub = Stub {
            packages: vec![
                ("a", "1.0.0", "Imports: b; Suggests: b, t"),
                ("b", "1.0.0", ""),
                ("t", "1.0.0", ""),
            ],
        };

        let tree = dep_tree(&stub, "a", "latest", true, false).unwrap();
        assert_eq!(
            lines(&tree),
            vec!["└── b 1.0.0", "[Suggests]", "└── t 1.0.0"]
        );
    }

    #[test]
    fn a_section_heading_is_colored_not_dimmed() {
        let stub = Stub {
            packages: vec![("a", "1.0.0", "Suggests: t"), ("t", "1.0.0", "")],
        };

        let tree = dep_tree(&stub, "a", "latest", true, false).unwrap();
        let out = render_tree(&tree, true);
        let heading = out.lines().find(|l| l.contains("Suggests")).unwrap();
        // Magenta (`35`), so the heading stands out from the dimmed glyphs
        // around it, but not bold (`1`) — the color carries it on its own.
        assert!(
            heading.contains("\u{1b}[35m[Suggests]\u{1b}[39m"),
            "{:?}",
            heading
        );
        assert!(!heading.contains("\u{1b}[1m"), "{:?}", heading);

        // ... and nothing at all when color is off.
        assert!(!render_tree(&tree, false).contains('\u{1b}'));
    }

    #[test]
    fn type_marks_flag_depends_and_linkingto_only() {
        assert_eq!(type_marks(&[]), None);
        assert_eq!(type_marks(&[RDepType::Imports]), None);
        // The soft types get a section of their own instead of a mark.
        assert_eq!(type_marks(&[RDepType::Suggests]), None);
        assert_eq!(type_marks(&[RDepType::Enhances]), None);
        assert_eq!(type_marks(&[RDepType::Depends]), Some("[D]".to_string()));
        assert_eq!(
            type_marks(&[RDepType::Imports, RDepType::LinkingTo]),
            Some("[L]".to_string())
        );
        assert_eq!(
            type_marks(&[RDepType::Depends, RDepType::LinkingTo]),
            Some("[DL]".to_string())
        );
    }
}

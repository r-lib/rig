//! `rig pkg tree`: the dependencies of a package, as a tree.
//!
//! The same transitive closure [`super::deps`] prints as a flat table, but laid
//! out so the shape of the graph is visible: each package's dependencies are
//! shown once, under its first occurrence, and later occurrences are leaves
//! marked `(*)`. That keeps the output readable — a popular package like `rlang`
//! is in most of the closure — and makes dependency cycles terminate on their
//! own.
//!
//! `--why` prints the same closure the other way around: the named package is
//! the root and the tree grows towards the packages that need it, down to the
//! queried package, which becomes a leaf. See [`invert_tree`].

use std::collections::{HashMap, HashSet};
use std::env;
use std::error::Error;
use std::fmt::Write;
use std::io::IsTerminal;

use clap::ArgMatches;
use simple_error::*;

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
    let why = args.get_one::<String>("why").map(|s| s.as_str());
    let json = args.get_flag("json") || pkgargs.get_flag("json") || mainargs.get_flag("json");

    let loader = DbSourcePackageLoader::new()?;
    let tree = dep_tree(&loader, &package, &ver, dev, no_base)?;
    let tree = match why {
        Some(target) => invert_tree(&tree, target, dev, no_base)?,
        None => tree,
    };

    print_tree(&tree, json)
}

/// `rig proj tree`: the dependency tree of a project's manifest.
///
/// The root is the project itself, which is not a package in the repositories,
/// so the walk starts from the dependency list `crate::proj::proj_read_deps`
/// read from the manifest. That list has already had the soft dependencies
/// dropped unless `--dev`, so the walk takes it as it is — the same `dev = true`
/// `rig proj deps --recursive` passes to [`super::deps::walk_deps`].
///
/// `dev` is only what the caller passed on the command line, for the hint of the
/// `why` error message; the walk itself is driven by `root_deps`.
pub(crate) fn proj_tree(
    root_name: &str,
    root_version: &RPackageVersion,
    root_deps: &[DepVersionSpec],
    dev: bool,
    no_base: bool,
    why: Option<&str>,
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
    let tree = match why {
        Some(target) => invert_tree(&tree, target, dev, no_base)?,
        None => tree,
    };

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
    /// The package `--why` asked about, if the tree is an inverted one. The
    /// children of a node are then the packages that need it, and every node's
    /// `types` and `requires` describe how it needs its *parent*.
    why: Option<String>,
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
/// different dependencies — is not honored. `rig proj lock` is what a full,
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
        why: None,
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
// Inverting the tree

/// An edge of the closure, seen from the package that is depended on: who needs
/// it, and how.
#[derive(Debug, Clone)]
struct Dependent {
    name: String,
    types: Vec<RDepType>,
    requires: Vec<String>,
}

/// Turn a tree upside down, so that `target` is the root and the tree grows
/// towards the packages that need it, down to the queried package or the
/// project, which has nothing above it and so becomes a leaf.
///
/// This is `--why`: not a repository-wide reverse dependency query, but the same
/// closure the tree already covers, read in the other direction. So `--dev`,
/// `--no-base` and `--version` mean what they mean for the tree itself, and no
/// further metadata is needed — a forward tree already contains every edge of
/// its closure, see [`collect_edges`].
///
/// Each node's `types` and `requires` are the edge *up* to the node above it,
/// i.e. how this package needs the one it is printed under. [`node_label`]
/// spells that out with a `needs` prefix, as the requirement is not a
/// requirement on the package the line names.
fn invert_tree(
    tree: &DepTree,
    target: &str,
    dev: bool,
    no_base: bool,
) -> Result<DepTree, Box<dyn Error>> {
    let mut parents: HashMap<String, Vec<Dependent>> = HashMap::new();
    let mut versions: HashMap<String, Option<RPackageVersion>> = HashMap::new();
    collect_edges(&tree.root, &mut parents, &mut versions);
    // A project is not a package in the repositories, but the database may well
    // know a *different* package by the same name. The manifest wins, so the
    // project's leaf line shows the version the manifest declares.
    versions.insert(tree.root.name.clone(), tree.root.version.clone());

    // Every package of the closure is a node of the forward tree, and so is the
    // root, which has no parents but is still a legal — if degenerate — target.
    let version = match versions.get(target) {
        Some(version) => version.clone(),
        None => bail!("{}", not_in_tree(tree, target, &versions, dev, no_base)),
    };

    let mut expanded: HashSet<String> = HashSet::new();
    expanded.insert(target.to_string());
    let mut seen: HashSet<String> = HashSet::new();
    let children = dependents_of(target, &parents, &versions, &mut expanded, &mut seen);

    let mut inverted = DepTree {
        root: TreeNode {
            name: target.to_string(),
            version,
            types: vec![],
            requires: vec![],
            children,
            repeat: false,
        },
        total: seen.len(),
        why: Some(target.to_string()),
    };
    prune_empty_repeats(&mut inverted.root);

    Ok(inverted)
}

/// Collect every edge of a tree, keyed by the package that is depended on, and
/// the version of every package in it.
///
/// A forward tree holds the whole edge set of its closure: [`children_of`] makes
/// a node for every dependency, including the ones that end up as `(*)` leaves,
/// and expands each package exactly once, so each package's outgoing edges are
/// in the tree exactly once. Base packages are never expanded, which is why they
/// can only ever be the root of an inverted tree, never a node inside one.
fn collect_edges(
    node: &TreeNode,
    parents: &mut HashMap<String, Vec<Dependent>>,
    versions: &mut HashMap<String, Option<RPackageVersion>>,
) {
    versions
        .entry(node.name.clone())
        .or_insert_with(|| node.version.clone());

    for child in node.children.iter() {
        let dependents = parents.entry(child.name.clone()).or_default();
        // The database's dependency lists are not simplified, so one package can
        // list another twice, e.g. under both `Imports` and `LinkingTo`. Merge
        // those into one edge rather than printing the dependent twice.
        match dependents.iter_mut().find(|d| d.name == node.name) {
            Some(edge) => {
                for t in child.types.iter() {
                    if !edge.types.contains(t) {
                        edge.types.push(t.clone());
                    }
                }
                for req in child.requires.iter() {
                    if !edge.requires.contains(req) {
                        edge.requires.push(req.clone());
                    }
                }
            }
            None => dependents.push(Dependent {
                name: node.name.clone(),
                types: child.types.clone(),
                requires: child.requires.clone(),
            }),
        }
        collect_edges(child, parents, versions);
    }
}

/// The nodes below a package in an inverted tree: the packages that need it,
/// each expanded the first time it is seen.
///
/// The mirror image of [`children_of`], down to sorting the nodes the way they
/// are printed before descending into them, so that a `(*)` always points at a
/// line above it.
fn dependents_of(
    package: &str,
    parents: &HashMap<String, Vec<Dependent>>,
    versions: &HashMap<String, Option<RPackageVersion>>,
    expanded: &mut HashSet<String>,
    seen: &mut HashSet<String>,
) -> Vec<TreeNode> {
    let mut wanted: Vec<&Dependent> = match parents.get(package) {
        Some(dependents) => dependents.iter().collect(),
        None => return vec![],
    };

    wanted.sort_by(|a, b| {
        r_first(&a.name).cmp(&r_first(&b.name)).then_with(|| {
            type_rank(&a.types)
                .cmp(&type_rank(&b.types))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        })
    });

    let mut nodes: Vec<TreeNode> = vec![];
    for dependent in wanted {
        seen.insert(dependent.name.clone());
        let mut node = TreeNode {
            name: dependent.name.clone(),
            version: versions.get(&dependent.name).cloned().flatten(),
            types: dependent.types.clone(),
            requires: dependent.requires.clone(),
            children: vec![],
            repeat: false,
        };

        if !expanded.insert(dependent.name.clone()) {
            node.repeat = true;
            nodes.push(node);
            continue;
        }

        node.children = dependents_of(&dependent.name, parents, versions, expanded, seen);
        nodes.push(node);
    }

    nodes
}

/// The error for a `--why` package that is not in the tree, with a hint about
/// the flag that would most likely have brought it in.
fn not_in_tree(
    tree: &DepTree,
    target: &str,
    versions: &HashMap<String, Option<RPackageVersion>>,
    dev: bool,
    no_base: bool,
) -> String {
    let mut msg = format!(
        "Package '{}' is not in the dependency tree of '{}'.",
        target, tree.root.name
    );

    if no_base && is_base_package(target) {
        msg.push_str(" --no-base leaves out R and the base packages.");
    } else if let Some(name) = versions
        .keys()
        .find(|name| name.to_lowercase() == target.to_lowercase())
    {
        // R package names are case sensitive, so this is an easy mistake to
        // make and a cheap one to point at.
        msg.push_str(&format!(" Did you mean '{}'?", name));
    } else if !dev {
        msg.push_str(" --dev also follows Suggests and Enhances.");
    }

    msg
}

// ------------------------------------------------------------------------
// Output

/// Render the tree the way it is printed on a terminal: a colored header line
/// naming the package version, how many direct dependencies it has and how many
/// packages there are altogether, then the tree itself.
///
/// An inverted tree counts dependents instead, and its `total` includes the
/// queried package, which is a leaf of it.
fn render_tree(tree: &DepTree, color: bool) -> String {
    use owo_colors::OwoColorize;

    let root = &tree.root;
    let direct = root.children.len();
    let tag = if tree.why.is_some() {
        format!(
            "{} direct dependent{}, {} total",
            direct,
            if direct == 1 { "" } else { "s" },
            tree.total
        )
    } else {
        format!("{} direct, {} total", direct, tree.total)
    };

    // R and the base packages have no version of their own, so the header is
    // just the name; everything else has a version, or `?` if the repositories
    // do not know it.
    let name = match version_cell(root) {
        Some(version) if color => format!("{} {}", root.name.cyan().bold(), version.bold()),
        Some(version) => format!("{} {}", root.name, version),
        None if color => root.name.cyan().bold().to_string(),
        None => root.name.clone(),
    };

    let mut out = String::new();
    if color {
        let _ = writeln!(out, "{} — {}", name, tag.dimmed());
    } else {
        let _ = writeln!(out, "{} — {}", name, tag);
    }
    // No blank line between the header and the tree: the header *is* the root
    // node, so the `├──` glyphs below have to connect to it.
    render_children(&root.children, "", tree.why.is_some(), color, &mut out);
    out
}

/// The version a node's line shows: nothing for R and the base packages, which
/// ship with R and so have no version of their own, otherwise what the
/// repositories offer, or `?` if they do not know the package.
fn version_cell(node: &TreeNode) -> Option<String> {
    if node.version.is_some() || !is_base_package(&node.name) {
        Some(version_cell_for(&node.name, node.version.as_ref()))
    } else {
        None
    }
}

/// Render the children of a node, each on its own line, with the box-drawing
/// prefix that connects it to its parent.
///
/// The soft dependencies go into their own `[Suggests]` / `[Enhances]` sections,
/// the way `cargo tree` sets `[dev-dependencies]` apart, each section numbering
/// its own lines. In practice that only happens under the queried package, as
/// `--dev` does not apply below it.
///
/// An `inverted` tree has no sections: a soft edge there is one line deep inside
/// the tree, where a heading would both read wrong and interrupt the box
/// drawing, so [`type_marks`] marks it inline instead.
fn render_children(
    children: &[TreeNode],
    prefix: &str,
    inverted: bool,
    color: bool,
    out: &mut String,
) {
    use owo_colors::OwoColorize;

    for (section, group) in sections(children, inverted) {
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
            let label = node_label(child, inverted, color);
            if color {
                let _ = writeln!(out, "{}{}", glyphs.dimmed(), label);
            } else {
                let _ = writeln!(out, "{}{}", glyphs, label);
            }
            render_children(
                &child.children,
                &format!("{}{}", prefix, below),
                inverted,
                color,
                out,
            );
        }
    }
}

/// Split a node's children into the sections they are printed in: first the
/// hard dependencies, unlabelled, then one section per soft dependency type.
///
/// The children are already sorted by [`type_rank`], so each section is a
/// contiguous run and this only has to find the boundaries.
///
/// An inverted tree is one section: see [`render_children`].
fn sections(children: &[TreeNode], inverted: bool) -> Vec<(Option<RDepType>, &[TreeNode])> {
    if inverted {
        return if children.is_empty() {
            vec![]
        } else {
            vec![(None, children)]
        };
    }

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
///
/// In an inverted tree the requirement and the marks belong to the edge *up* to
/// the line above, not to the package the line names, so the requirement is
/// written `(needs >= 1.0.2)`: it is dplyr that needs cli `>= 3.4.0`, printed
/// next to dplyr's own version.
fn node_label(node: &TreeNode, inverted: bool, color: bool) -> String {
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

    if let Some(version) = version_cell(node) {
        dim(version);
    }
    if !node.requires.is_empty() {
        let requires = node.requires.join(", ");
        dim(if inverted {
            format!("(needs {})", requires)
        } else {
            format!("({})", requires)
        });
    }
    if let Some(marks) = type_marks(&node.types, inverted) {
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
/// `Imports` is the common case and stays unmarked. In a forward tree the soft
/// types are not marked either, they have a section of their own; an inverted
/// tree has no sections, so they are marked here, `[S]` and `[E]`.
fn type_marks(types: &[RDepType], inverted: bool) -> Option<String> {
    let mut marks = String::new();
    if types.contains(&RDepType::Depends) {
        marks.push('D');
    }
    if types.contains(&RDepType::LinkingTo) {
        marks.push('L');
    }
    if inverted {
        if types.contains(&RDepType::Suggests) {
            marks.push('S');
        }
        if types.contains(&RDepType::Enhances) {
            marks.push('E');
        }
    }
    if marks.is_empty() {
        None
    } else {
        Some(format!("[{}]", marks))
    }
}

/// Print the tree as a single nested JSON object. A `repeat` node always has an
/// empty `dependencies` array; its dependencies are under its first occurrence.
///
/// An inverted tree is a different shape and says so: `"inverted": true`, the
/// package `--why` asked about in `why`, and `dependents` /
/// `direct_dependents` instead of `dependencies` / `direct`, so that a consumer
/// cannot read the arrows the wrong way around.
fn print_tree_json(tree: &DepTree) -> Result<(), Box<dyn Error>> {
    println!("{}", tree_json(tree)?);
    Ok(())
}

fn tree_json(tree: &DepTree) -> Result<String, Box<dyn Error>> {
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

    #[derive(serde::Serialize)]
    struct InvertedNode<'a> {
        package: &'a str,
        version: Option<String>,
        types: Vec<String>,
        requires: &'a [String],
        repeat: bool,
        dependents: Vec<InvertedNode<'a>>,
    }

    #[derive(serde::Serialize)]
    struct InvertedRoot<'a> {
        package: &'a str,
        version: Option<String>,
        inverted: bool,
        why: &'a str,
        direct_dependents: usize,
        total: usize,
        dependents: Vec<InvertedNode<'a>>,
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

    fn inverted_nodes(children: &[TreeNode]) -> Vec<InvertedNode<'_>> {
        children
            .iter()
            .map(|child| InvertedNode {
                package: &child.name,
                version: child.version.as_ref().map(|v| v.to_string()),
                types: child.types.iter().map(|t| t.to_string()).collect(),
                requires: &child.requires,
                repeat: child.repeat,
                dependents: inverted_nodes(&child.children),
            })
            .collect()
    }

    let root = &tree.root;
    let out = match tree.why.as_deref() {
        Some(why) => serde_json::to_string_pretty(&InvertedRoot {
            package: &root.name,
            version: root.version.as_ref().map(|v| v.to_string()),
            inverted: true,
            why,
            direct_dependents: root.children.len(),
            total: tree.total,
            dependents: inverted_nodes(&root.children),
        })?,
        None => serde_json::to_string_pretty(&Root {
            package: &root.name,
            version: root.version.as_ref().map(|v| v.to_string()),
            direct: root.children.len(),
            total: tree.total,
            dependencies: nodes(&root.children),
        })?,
    };

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dcf::PackageDependencies;
    use crate::pkg::stub::{stub_deps, Stub};

    /// The tree as `(indent, label)` pairs, so a test can assert on the shape
    /// without the box-drawing noise.
    fn shape(tree: &DepTree) -> Vec<(usize, String)> {
        fn walk(nodes: &[TreeNode], depth: usize, inverted: bool, out: &mut Vec<(usize, String)>) {
            for node in nodes {
                out.push((depth, node_label(node, inverted, false)));
                walk(&node.children, depth + 1, inverted, out);
            }
        }
        let mut out = vec![];
        walk(&tree.root.children, 0, tree.why.is_some(), &mut out);
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
    // Inverting the tree, i.e. `--why`

    /// The tree of `package`, inverted around `target`, the way
    /// `rig pkg tree <package> --why <target>` builds it.
    fn why_tree(stub: &Stub, package: &str, target: &str, dev: bool) -> DepTree {
        let tree = dep_tree(stub, package, "latest", dev, false).unwrap();
        invert_tree(&tree, target, dev, false).unwrap()
    }

    #[test]
    fn why_inverts_a_diamond() {
        let stub = Stub {
            packages: vec![
                ("a", "1.0.0", "Imports: b, c"),
                ("b", "1.1.0", "Imports: d"),
                ("c", "1.2.0", "Imports: d"),
                ("d", "1.3.0", ""),
            ],
        };

        let tree = why_tree(&stub, "a", "d", false);

        assert_eq!(tree.root.name, "d");
        assert_eq!(tree.root.version.as_ref().unwrap().original, "1.3.0");
        assert_eq!(tree.why.as_deref(), Some("d"));
        assert_eq!(tree.total, 3);
        assert_eq!(
            shape(&tree),
            vec![
                (0, "b 1.1.0".to_string()),
                // The queried package is a leaf, and it has no dependents of
                // its own to elide, so it is not marked `(*)`.
                (1, "a 1.0.0".to_string()),
                (0, "c 1.2.0".to_string()),
                (1, "a 1.0.0".to_string()),
            ]
        );
    }

    #[test]
    fn why_shows_the_edge_up_to_the_line_above() {
        let stub = Stub {
            packages: vec![
                ("a", "1.0.0", "Imports: b (>= 1.5.0), c"),
                ("b", "1.1.0", "Imports: d (>= 2.0.0); LinkingTo: d"),
                ("c", "1.2.0", "Imports: d"),
                ("d", "1.3.0", ""),
            ],
        };

        let tree = why_tree(&stub, "a", "d", false);

        // Every line says how *that* package needs the one above it: `b` needs
        // `d` at `>= 2.0.0` and links to it, `c` needs it unconstrained, and
        // `a` needs `b` at `>= 1.5.0` but `c` unconstrained.
        assert_eq!(
            shape(&tree),
            vec![
                (0, "b 1.1.0 (needs >= 2.0.0) [L]".to_string()),
                (1, "a 1.0.0 (needs >= 1.5.0)".to_string()),
                (0, "c 1.2.0".to_string()),
                (1, "a 1.0.0".to_string()),
            ]
        );
    }

    #[test]
    fn why_dedupes_with_the_repeat_marker() {
        let stub = Stub {
            packages: vec![
                ("a", "1.0.0", "Imports: x"),
                ("x", "1.1.0", "Imports: b, c"),
                ("b", "1.2.0", "Imports: d"),
                ("c", "1.3.0", "Imports: d"),
                ("d", "1.4.0", ""),
            ],
        };

        let tree = why_tree(&stub, "a", "d", false);

        assert_eq!(tree.total, 4);
        assert_eq!(
            shape(&tree),
            vec![
                (0, "b 1.2.0".to_string()),
                (1, "x 1.1.0".to_string()),
                (2, "a 1.0.0".to_string()),
                (0, "c 1.3.0".to_string()),
                // `x` is expanded under `b`, above.
                (1, "x 1.1.0 (*)".to_string()),
            ]
        );
    }

    #[test]
    fn why_counts_the_direct_dependents() {
        let stub = Stub {
            packages: vec![
                ("a", "1.0.0", "Imports: b, c"),
                ("b", "1.1.0", "Imports: d"),
                ("c", "1.2.0", "Imports: d"),
                ("d", "1.3.0", ""),
            ],
        };

        let header = |target: &str| {
            render_tree(&why_tree(&stub, "a", target, false), false)
                .lines()
                .next()
                .unwrap()
                .to_string()
        };

        assert_eq!(header("d"), "d 1.3.0 — 2 direct dependents, 3 total");
        // Singular, unlike the adjective "direct" of the forward tree.
        assert_eq!(header("b"), "b 1.1.0 — 1 direct dependent, 1 total");
    }

    #[test]
    fn why_a_base_package_can_be_the_root() {
        let stub = Stub {
            packages: vec![
                ("a", "1.0.0", "Depends: R (>= 3.5.0); Imports: b"),
                ("b", "1.1.0", "Imports: utils"),
            ],
        };

        // Base packages ship with R, so the header is the name alone, with no
        // version and no stray separator.
        let tree = why_tree(&stub, "a", "utils", false);
        assert_eq!(
            render_tree(&tree, false).lines().next().unwrap(),
            "utils — 1 direct dependent, 2 total"
        );
        assert_eq!(
            shape(&tree),
            vec![(0, "b 1.1.0".to_string()), (1, "a 1.0.0".to_string())]
        );

        // Base packages are never expanded in the forward tree, so they never
        // have dependencies of their own to invert.
        let tree = why_tree(&stub, "a", "R", false);
        assert_eq!(
            render_tree(&tree, false).lines().next().unwrap(),
            "R — 1 direct dependent, 1 total"
        );
        assert_eq!(
            shape(&tree),
            vec![(0, "a 1.0.0 (needs >= 3.5.0) [D]".to_string())]
        );
    }

    #[test]
    fn why_a_package_missing_from_the_database_can_be_the_target() {
        let stub = Stub {
            packages: vec![("a", "1.0.0", "Imports: gone")],
        };

        let tree = why_tree(&stub, "a", "gone", false);
        assert_eq!(
            render_tree(&tree, false).lines().next().unwrap(),
            "gone ? — 1 direct dependent, 1 total"
        );
    }

    #[test]
    fn why_a_cycle_terminates() {
        let stub = Stub {
            packages: vec![
                ("a", "1.0.0", "Imports: b"),
                ("b", "1.0.0", "Imports: c"),
                ("c", "1.0.0", "Imports: b, a"),
            ],
        };

        let tree = why_tree(&stub, "a", "b", false);
        assert_eq!(tree.total, 3);
        assert_eq!(
            shape(&tree),
            vec![
                (0, "a 1.0.0".to_string()),
                (1, "c 1.0.0".to_string()),
                (2, "b 1.0.0 (*)".to_string()),
                (0, "c 1.0.0 (*)".to_string()),
            ]
        );
    }

    #[test]
    fn why_a_self_dependency_terminates() {
        let stub = Stub {
            packages: vec![("a", "1.0.0", "Imports: a, b"), ("b", "1.0.0", "")],
        };

        let tree = why_tree(&stub, "a", "a", false);
        assert_eq!(shape(&tree), vec![(0, "a 1.0.0 (*)".to_string())]);
    }

    #[test]
    fn why_the_queried_package_itself_is_an_empty_tree() {
        let stub = Stub {
            packages: vec![("a", "1.0.0", "Imports: b"), ("b", "1.0.0", "")],
        };

        let tree = why_tree(&stub, "a", "a", false);
        assert_eq!(
            render_tree(&tree, false),
            "a 1.0.0 — 0 direct dependents, 0 total\n"
        );
    }

    #[test]
    fn why_marks_a_soft_edge_inline() {
        let stub = Stub {
            packages: vec![
                ("a", "1.0.0", "Imports: b; Suggests: t"),
                ("b", "1.1.0", ""),
                ("t", "1.2.0", "Imports: ti"),
                ("ti", "1.3.0", ""),
            ],
        };

        let tree = why_tree(&stub, "a", "ti", true);
        assert_eq!(
            shape(&tree),
            vec![(0, "t 1.2.0".to_string()), (1, "a 1.0.0 [S]".to_string())]
        );
        // An inverted tree has no `[Suggests]` section: the heading would be
        // deep inside the tree, where it reads wrong and breaks the glyphs.
        assert_eq!(
            lines(&tree),
            vec!["└── t 1.2.0".to_string(), "    └── a 1.0.0 [S]".to_string()]
        );
    }

    #[test]
    fn why_the_project_root_is_a_leaf_with_the_manifest_version() {
        let stub = Stub {
            packages: vec![
                // A different package that happens to have the project's name.
                ("myproj", "0.0.1", "Imports: neverseen"),
                ("b", "1.0.0", "Imports: myproj"),
                ("neverseen", "1.0.0", ""),
            ],
        };

        let tree = project_tree(&stub, "Imports: b", false);
        let tree = invert_tree(&tree, "myproj", false, false).unwrap();

        // The manifest's version wins over the database's, in the header and on
        // the leaf line alike.
        assert_eq!(
            render_tree(&tree, false).lines().next().unwrap(),
            "myproj 0.1.0 — 1 direct dependent, 2 total"
        );
        assert_eq!(
            shape(&tree),
            vec![
                (0, "b 1.0.0".to_string()),
                (1, "myproj 0.1.0 (*)".to_string()),
            ]
        );
    }

    #[test]
    fn why_merges_an_edge_the_database_lists_twice() {
        // The database's dependency lists are not simplified, so one package can
        // list another under two dependency types as two separate entries.
        let mut deps = PackageDependencies::from_str("d (>= 2.0.0)", "Imports").unwrap();
        deps.append(&mut PackageDependencies::from_str("d", "LinkingTo").unwrap());
        let stub = Stub {
            packages: vec![("a", "1.0.0", "Imports: b"), ("d", "1.3.0", "")],
        };
        let forward = tree_from_deps(
            &stub,
            "b",
            Some(RPackageVersion::from_str("1.1.0").unwrap()),
            &deps.dependencies,
            false,
            false,
        );

        let tree = invert_tree(&forward, "d", false, false).unwrap();
        // One line for `b`, not two, with both types and the requirement.
        assert_eq!(
            shape(&tree),
            vec![(0, "b 1.1.0 (needs >= 2.0.0) [L]".to_string())]
        );
    }

    #[test]
    fn why_an_unknown_package_is_an_error() {
        let stub = Stub {
            packages: vec![("a", "1.0.0", "Imports: b"), ("b", "1.0.0", "")],
        };
        let tree = dep_tree(&stub, "a", "latest", false, false).unwrap();

        let err = invert_tree(&tree, "nosuchpkg", false, false)
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "Package 'nosuchpkg' is not in the dependency tree of 'a'. \
             --dev also follows Suggests and Enhances."
        );

        // With `--dev` there is nothing left to suggest.
        let tree = dep_tree(&stub, "a", "latest", true, false).unwrap();
        let err = invert_tree(&tree, "nosuchpkg", true, false)
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "Package 'nosuchpkg' is not in the dependency tree of 'a'."
        );
    }

    #[test]
    fn why_points_at_a_package_that_differs_only_in_case() {
        let stub = Stub {
            packages: vec![("a", "1.0.0", "Imports: cpp11"), ("cpp11", "1.0.0", "")],
        };
        let tree = dep_tree(&stub, "a", "latest", false, false).unwrap();

        let err = invert_tree(&tree, "Cpp11", false, false)
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "Package 'Cpp11' is not in the dependency tree of 'a'. Did you mean 'cpp11'?"
        );
    }

    #[test]
    fn why_a_base_package_says_what_no_base_left_out() {
        let stub = Stub {
            packages: vec![("a", "1.0.0", "Imports: b, utils"), ("b", "1.0.0", "")],
        };
        let tree = dep_tree(&stub, "a", "latest", false, true).unwrap();

        let err = invert_tree(&tree, "utils", false, true)
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "Package 'utils' is not in the dependency tree of 'a'. \
             --no-base leaves out R and the base packages."
        );
    }

    #[test]
    fn why_json_says_that_it_is_inverted() {
        let stub = Stub {
            packages: vec![
                ("a", "1.0.0", "Imports: b"),
                ("b", "1.1.0", "Imports: d (>= 2.0.0)"),
                ("d", "1.3.0", ""),
            ],
        };

        let json: serde_json::Value =
            serde_json::from_str(&tree_json(&why_tree(&stub, "a", "d", false)).unwrap()).unwrap();

        assert_eq!(json["package"], "d");
        assert_eq!(json["version"], "1.3.0");
        assert_eq!(json["inverted"], true);
        assert_eq!(json["why"], "d");
        assert_eq!(json["direct_dependents"], 1);
        assert_eq!(json["total"], 2);
        assert!(json.get("dependencies").is_none());
        assert!(json.get("direct").is_none());

        let b = &json["dependents"][0];
        assert_eq!(b["package"], "b");
        assert_eq!(b["requires"][0], ">= 2.0.0");
        assert_eq!(b["dependents"][0]["package"], "a");

        // The forward tree's JSON is unchanged.
        let forward = tree_json(&dep_tree(&stub, "a", "latest", false, false).unwrap()).unwrap();
        let forward: serde_json::Value = serde_json::from_str(&forward).unwrap();
        assert_eq!(forward["direct"], 1);
        assert_eq!(forward["dependencies"][0]["package"], "b");
        assert!(forward.get("inverted").is_none());
        assert!(forward.get("dependents").is_none());
    }

    #[test]
    fn every_package_of_the_tree_can_be_asked_about() {
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
            let forward = dep_tree(&stub, "a", "latest", dev, false).unwrap();
            let closure = names(&forward.root);

            let mut targets: Vec<String> = closure.iter().cloned().collect();
            targets.push("a".to_string());
            targets.sort();

            for target in targets.iter() {
                let tree = invert_tree(&forward, target, dev, false)
                    .unwrap_or_else(|e| panic!("--why {} (dev = {}): {}", target, dev, e));

                // Everything in an inverted tree is in the forward closure...
                for name in names(&tree.root) {
                    assert!(
                        closure.contains(&name) || name == "a",
                        "--why {} (dev = {}) showed {}",
                        target,
                        dev,
                        name
                    );
                }
                // ... and every branch of it ends at the queried package or at
                // a `(*)`, i.e. nothing is left dangling.
                assert!(
                    leaves(&tree.root)
                        .iter()
                        .all(|leaf| leaf.name == "a" || leaf.repeat),
                    "--why {} (dev = {}): {:?}",
                    target,
                    dev,
                    leaves(&tree.root)
                );
            }
        }
    }

    /// The nodes of a tree that have no children.
    fn leaves(root: &TreeNode) -> Vec<&TreeNode> {
        fn walk<'a>(nodes: &'a [TreeNode], out: &mut Vec<&'a TreeNode>) {
            for node in nodes {
                if node.children.is_empty() {
                    out.push(node);
                }
                walk(&node.children, out);
            }
        }
        let mut out = vec![];
        walk(&root.children, &mut out);
        out
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
        assert_eq!(type_marks(&[], false), None);
        assert_eq!(type_marks(&[RDepType::Imports], false), None);
        // The soft types get a section of their own instead of a mark.
        assert_eq!(type_marks(&[RDepType::Suggests], false), None);
        assert_eq!(type_marks(&[RDepType::Enhances], false), None);
        assert_eq!(
            type_marks(&[RDepType::Depends], false),
            Some("[D]".to_string())
        );
        assert_eq!(
            type_marks(&[RDepType::Imports, RDepType::LinkingTo], false),
            Some("[L]".to_string())
        );
        assert_eq!(
            type_marks(&[RDepType::Depends, RDepType::LinkingTo], false),
            Some("[DL]".to_string())
        );
    }

    #[test]
    fn type_marks_flag_the_soft_types_in_an_inverted_tree() {
        // An inverted tree has no `[Suggests]` section to put them in.
        assert_eq!(
            type_marks(&[RDepType::Suggests], true),
            Some("[S]".to_string())
        );
        assert_eq!(
            type_marks(&[RDepType::Enhances], true),
            Some("[E]".to_string())
        );
        assert_eq!(type_marks(&[RDepType::Imports], true), None);
        assert_eq!(
            type_marks(&[RDepType::Depends, RDepType::Suggests], true),
            Some("[DS]".to_string())
        );
    }
}

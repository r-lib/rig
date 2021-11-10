
use std::path::Path;
use regex::Regex;
use std::fs::File;
use std::io::{prelude::*, BufReader};

pub fn basename(path: &str) -> Option<&str> {
    path.rsplitn(2, '/').next()
}

pub fn read_lines(path: &Path) -> Result<Vec<String>, std::io::Error> {
    let file = File::open(path)?;
    let buf = BufReader::new(file);
    let lines = buf.lines()
        .map(|l| l.expect("Could not parse line"))
        .collect();
    Ok(lines)
}

pub fn grep_lines(re: &Regex, lines: &Vec<String>) -> Vec<usize> {
    lines.iter().enumerate().filter_map(|record| {
        let (no, line) = record;
        if re.is_match(line) { Some(no) } else { None }
    }).collect()
}

pub fn replace_in_file(path: &Path, re: &Regex, sub: &str)
                       -> Result<(), std::io::Error> {
    let mut lines = read_lines(path)?;
    let mch = grep_lines(re, &lines);
    if mch.len() > 0 {
        println!("Updating {:?}", path);
        for m in mch {
            lines[m] = re.replace(&lines[m], sub).to_string();
        }
        let mut path2 = path.to_owned();
        let ext = path.extension()
            .unwrap_or_else(|| std::ffi::OsStr::new(""))
            .to_str().unwrap();
        path2.set_extension(ext.to_owned() + "bak");
        let mut f = File::create(&path2).expect("Unable to create file");
        for line in &lines {
            write!(f, "{}\n", line)?;
        }

        let perms = std::fs::metadata(path)?.permissions();
        std::fs::set_permissions(&path2, perms)?;
        std::fs::rename(path2, path)?;
    }

    Ok(())
}

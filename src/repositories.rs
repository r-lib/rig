use std::error::Error;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};

use csv::ReaderBuilder;
use serde::Serialize;

use crate::repos::*;

#[derive(Debug, Serialize)]
pub struct RepoFileEntry {
    pub name: String,
    pub description: String,
    pub url: String,
    pub default: bool,
    pub source: bool,
    pub win_binary: bool,
    pub mac_binary: bool,
}

#[derive(Debug, Serialize)]
pub struct RepositoriesContents {
    pub data: Vec<RepoFileEntry>,
    pub comments: Vec<(usize, String)>,
}

pub fn read_repositories_file(path: &str) -> Result<RepositoriesContents, Box<dyn Error>> {
    let tsv = read_tsv(path)?;
    let comments = tsv.0;
    let mut data = vec![];
    for row in tsv.2 {
        data.push(RepoFileEntry {
            name: row[0].clone(),
            description: row[1].clone(),
            url: row[2].clone(),
            default: row[3].to_lowercase() == "true",
            source: row[4].to_lowercase() == "true",
            win_binary: row[5].to_lowercase() == "true",
            mac_binary: row[6].to_lowercase() == "true",
        });
    }
    Ok(RepositoriesContents { data, comments })
}

pub fn write_repositories_file(
    repos: RepositoriesContents,
    path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let comments = repos.comments;
    let headers = vec![
        "menu_name".to_string(),
        "URL".to_string(),
        "default".to_string(),
        "source".to_string(),
        "win.binary".to_string(),
        "mac.binary".to_string(),
    ];
    let mut data = vec![];
    for entry in repos.data {
        data.push(vec![
            entry.name,
            entry.description,
            entry.url,
            entry.default.to_string().to_uppercase(),
            entry.source.to_string().to_uppercase(),
            entry.win_binary.to_string().to_uppercase(),
            entry.mac_binary.to_string().to_uppercase(),
        ]);
    }

    write_tsv(path, comments, Some(headers), data)?;

    Ok(())
}

pub fn comment_out_repository(repos: &mut RepositoriesContents, repo_name: &str) {
    // Find the repository by name
    let pos = repos.data.iter().position(|entry| entry.name == repo_name);

    if let Some(index) = pos {
        // Calculate what line number this entry would be at in the file
        // We need to simulate the write process to determine line numbers

        // Sort comments to process them in order
        let mut comments_sorted = repos.comments.clone();
        comments_sorted.sort_by_key(|(line_num, _)| *line_num);

        // Simulate file layout to find line numbers of data entries
        let mut current_line = 1;
        let mut data_index = 0;
        let total_data = repos.data.len() + 1; // +1 for header row
        let mut comment_idx = 0;
        let mut data_line_numbers = Vec::new();

        loop {
            // Check if there's a comment at the current line
            if comment_idx < comments_sorted.len() && comments_sorted[comment_idx].0 == current_line
            {
                comment_idx += 1;
                current_line += 1;
                continue;
            }

            // This line is for data (header or data row)
            if data_index < total_data {
                data_line_numbers.push(current_line);
                data_index += 1;
                current_line += 1;
            } else {
                break;
            }
        }

        // Remove the entry from data
        let entry = repos.data.remove(index);

        // The entry was at index+1 in data_line_numbers (index 0 is the header)
        let entry_line_number = data_line_numbers[index + 1];

        // Format the entry as a commented TSV line
        let fields = vec![
            entry.name,
            entry.description,
            entry.url,
            entry.default.to_string().to_uppercase(),
            entry.source.to_string().to_uppercase(),
            entry.win_binary.to_string().to_uppercase(),
            entry.mac_binary.to_string().to_uppercase(),
        ];
        let formatted = format_tsv_row(&fields).unwrap_or_else(|_| fields.join("\t"));
        let commented_line = format!("## {}", formatted);

        // Add the commented line at its original position
        repos.comments.push((entry_line_number, commented_line));

        // Sort comments by line number
        repos.comments.sort_by_key(|(line_num, _)| *line_num);
    }
}

pub fn add_repository(repos: &mut RepositoriesContents, entry: &RepoEntry) {
    comment_out_repository(repos, &entry.name);
    let new_entry = RepoFileEntry {
        name: entry.name.clone(),
        description: entry.name.clone(),
        url: entry.url.clone(),
        default: true,
        source: true,
        win_binary: true,
        mac_binary: true,
    };
    repos.data.push(new_entry);
}

pub fn add_repositories_comment(repos: &mut RepositoriesContents, comment: &str) {
    let total_lines = repos.comments.len() + repos.data.len();
    repos
        .comments
        .push((total_lines + 2, "## ".to_string() + comment));
}

fn read_tsv(
    path: &str,
) -> Result<(Vec<(usize, String)>, Option<Vec<String>>, Vec<Vec<String>>), Box<dyn std::error::Error>>
{
    let file = File::open(path)?;
    let reader = BufReader::new(file);

    let mut comments = Vec::new();
    let mut data_lines = Vec::new();

    for (line_number, line) in reader.lines().enumerate() {
        let line = line?;
        let trimmed = line.trim();

        if trimmed.starts_with('#') {
            comments.push((line_number + 1, line));
        } else if trimmed.is_empty() {
            comments.push((line_number + 1, String::new()));
        } else {
            data_lines.push(line);
        }
    }

    // Handle case where there are no data lines
    if data_lines.is_empty() {
        return Ok((comments, None, Vec::new()));
    }

    // Parse data lines with csv crate
    let data = data_lines.join("\n");
    let mut csv_reader = ReaderBuilder::new()
        .delimiter(b'\t')
        .has_headers(true)
        .flexible(true) // Allow variable number of columns
        .from_reader(data.as_bytes());

    // Extract headers
    let headers = csv_reader
        .headers()?
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();

    // Extract data rows
    let mut rows = Vec::new();
    for result in csv_reader.records() {
        let record = result?;
        rows.push(record.iter().map(|s| s.to_string()).collect());
    }

    Ok((comments, Some(headers), rows))
}

// Base R's repositories field is a TSV, but it still double quotes most fields with
// spaces (not all, though). So let's keep the same convention. 'CRAN (extras)' is not
// double quotes originally, but now we will quote it, but that should be fine.

fn quote_if_has_space(field: &str) -> String {
    if field.contains(' ') {
        // Escape internal quotes and wrap in quotes
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

fn format_tsv_row(fields: &[String]) -> Result<String, Box<dyn std::error::Error>> {
    let formatted = fields
        .iter()
        .map(|f| quote_if_has_space(f))
        .collect::<Vec<_>>()
        .join("\t");
    Ok(formatted)
}

fn write_tsv(
    path: &str,
    comments: Vec<(usize, String)>,
    headers: Option<Vec<String>>,
    data: Vec<Vec<String>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);

    // Sort comments by line number
    let mut comments = comments;
    comments.sort_by_key(|(line_num, _)| *line_num);

    // Create a map of line numbers to comments
    let comment_map: std::collections::HashMap<usize, &String> =
        comments.iter().map(|(num, text)| (*num, text)).collect();

    let total_lines = comments.len() + data.len() + (if headers.is_some() { 1 } else { 0 });

    let mut data_iter = data.iter();
    let mut header_written = false;

    for line_num in 1..=total_lines {
        if let Some(comment_or_empty) = comment_map.get(&line_num) {
            // Write comment or empty line
            if comment_or_empty.is_empty() {
                writeln!(writer)?;
            } else {
                writeln!(writer, "{}", comment_or_empty)?;
            }
        } else {
            // Write header first if present and not yet written
            if !header_written && headers.is_some() {
                let header_line = format_tsv_row(headers.as_ref().unwrap())?;
                writeln!(writer, "{}", header_line)?;
                header_written = true;
            } else {
                // Write next data row
                if let Some(row) = data_iter.next() {
                    let line = format_tsv_row(row)?;
                    writeln!(writer, "{}", line)?;
                }
            }
        }
    }

    writer.flush()?;
    Ok(())
}

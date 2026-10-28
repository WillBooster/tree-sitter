use std::{fs, path::Path};

use anyhow::{Result, anyhow};
use regex::Regex;
use semver::Version;
use tree_sitter_generate::write_file;

pub(super) fn update_manifest(path: &Path, template: &str) -> Result<()> {
    let versions =
        Regex::new(r#"(\.(?:minimum_zig_version|@"minimum_zig_version")\s*=\s*")([^"]+)(")"#)?;
    let floor = versions.captures(template).unwrap();
    let minimum = Version::parse(&floor[2])?;
    let mut contents = fs::read_to_string(path)?;
    let tokens = Regex::new(
        r#"(?m)//[^\n]*|\\\\[^\n]*|"(?:\\.|[^"\\])*"|([{}])|\.(?:([A-Za-z_][A-Za-z_0-9]*)|@"((?:\\.|[^"\\])*)")\s*="#,
    )?;
    if let Some(version) = versions.captures_iter(&contents).find(|capture| {
        zig_manifest_path(&tokens, &contents, capture.get(0).unwrap().start())
            .is_some_and(|path| path == [None])
    }) {
        if Version::parse(&version[2])? < minimum {
            let value = version.get(2).unwrap().range();
            contents.replace_range(value, &floor[2]);
        }
    } else {
        let root = Regex::new(r"(?m)^[ \t]*\.\{")?
            .find(&contents)
            .ok_or_else(|| anyhow!("Invalid Zig package manifest: missing root struct"))?;
        contents.insert_str(
            root.end(),
            &format!("\n    .minimum_zig_version = \"{minimum}\","),
        );
    }
    contents = remove_legacy_zig_dependency(&tokens, &contents)?;
    write_file(path, contents)?;
    Ok(())
}

fn remove_legacy_zig_dependency(tokens: &Regex, contents: &str) -> Result<String> {
    let dependency = Regex::new(r#"\.(?:tree_sitter|@"tree_sitter")\s*=\s*\.\{"#)?;
    let legacy_url = Regex::new(
        r#"\.(?:url|@"url")\s*=\s*"git\+https://github\.com/tree-sitter/zig-tree-sitter#0cf58172e61f6fdd16f681cde42b4acb531a23db""#,
    )?;
    let mut removed = Vec::new();
    for field in dependency.find_iter(contents) {
        if zig_manifest_path(tokens, contents, field.start())
            .is_none_or(|path| path != [None, Some("dependencies")])
        {
            continue;
        }
        let end = zig_manifest_object_end(tokens, contents, field.end())?;
        let body = &contents[field.end()..end - 1];
        if legacy_url.find_iter(body).any(|url| {
            zig_manifest_path(tokens, body, url.start()).is_some_and(|path| path.is_empty())
        }) {
            removed.push(zig_dependency_removal(contents, field.start(), end));
        }
    }
    if removed.is_empty() {
        return Ok(contents.to_owned());
    }
    let mut updated = contents.to_owned();
    for range in removed.into_iter().rev() {
        updated.replace_range(range, "");
    }
    collapse_empty_zig_dependencies(tokens, &updated)
}

fn collapse_empty_zig_dependencies(tokens: &Regex, contents: &str) -> Result<String> {
    let empty = Regex::new(r#"(\.(?:dependencies|@"dependencies")\s*=\s*\.\{)\s*(\})"#)?;
    Ok(empty
        .replace_all(contents, |capture: &regex::Captures| {
            if zig_manifest_path(tokens, contents, capture.get(0).unwrap().start())
                .is_some_and(|path| path == [None])
            {
                format!("{}{}", &capture[1], &capture[2])
            } else {
                capture[0].to_owned()
            }
        })
        .into_owned())
}

fn zig_dependency_removal(contents: &str, field: usize, end: usize) -> std::ops::Range<usize> {
    let tail = &contents[end..];
    let separator = tail.trim_start_matches(char::is_whitespace);
    let suffix = if separator.starts_with(',') {
        tail.len() - separator.len() + 1
    } else {
        0
    };
    let remainder = &contents[end + suffix..];
    let comment = remainder.trim_start_matches([' ', '\t']);
    if comment.starts_with("//") {
        return field..end + suffix + remainder.len() - comment.len();
    }
    let suffix = suffix + usize::from(remainder.starts_with('\n'));
    let line = contents[..field].rfind('\n').map_or(0, |start| start + 1);
    let start = if contents[line..field].trim().is_empty() {
        line
    } else {
        field
    };
    start..end + suffix
}

fn zig_manifest_object_end(tokens: &Regex, contents: &str, start: usize) -> Result<usize> {
    let mut depth = 1;
    for token in tokens.captures_iter(&contents[start..]) {
        let Some(brace) = token.get(1) else {
            continue;
        };
        if brace.as_str() == "{" {
            depth += 1;
        } else {
            depth -= 1;
        }
        if depth == 0 {
            return Ok(start + brace.end());
        }
    }
    Err(anyhow!(
        "Invalid Zig package manifest: unterminated dependency"
    ))
}

fn zig_manifest_path<'a>(
    tokens: &Regex,
    contents: &'a str,
    end: usize,
) -> Option<Vec<Option<&'a str>>> {
    let mut path = Vec::new();
    let mut field = None;
    for token in tokens.captures_iter(contents) {
        let span = token.get(0).unwrap();
        if span.start() >= end {
            break;
        }
        if span.end() > end {
            return None;
        }
        if let Some(name) = token.get(2).or_else(|| token.get(3)) {
            field = Some(name.as_str());
        } else if let Some(brace) = token.get(1) {
            if brace.as_str() == "{" {
                path.push(field.take());
            } else {
                path.pop();
                field = None;
            }
        }
    }
    Some(path)
}

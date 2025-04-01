use atty::Stream;
use colored::*;
use regex::Regex;
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const WRAP_DIR: &str = "./wrappers";

struct Rule {
    regex: Regex,
    fg_color: Color,
    bg_color: Option<Color>,
}

fn get_wrapped_program() -> Option<String> {
    env::args()
        .nth(1)
        .map(|arg| {
            Path::new(&arg)
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.to_string())
        })
        .flatten()
}

/// Locate the real program in PATH (excluding wrappers directory)
fn find_real_program(program: &str) -> Option<PathBuf> {
    let path_var = env::var("PATH").ok()?;
    for dir in path_var.split(':') {
        if dir == WRAP_DIR {
            continue; // Skip our wrapper directory
        }
        let candidate = Path::new(dir).join(program);
        if candidate.exists()
            && candidate.is_file()
            && candidate.metadata().ok()?.permissions().mode() & 0o111 != 0
        {
            return Some(candidate);
        }
    }
    None
}

fn parse_color(color: &str) -> Color {
    match color.to_lowercase().as_str() {
        "red" => Color::Red,
        "blue" => Color::Blue,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "white" => Color::White,
        "black" => Color::Black,
        "brightred" => Color::BrightRed,
        "brightblue" => Color::BrightBlue,
        "brightgreen" => Color::BrightGreen,
        "brightyellow" => Color::BrightYellow,
        "brightmagenta" => Color::BrightMagenta,
        "brightcyan" => Color::BrightCyan,
        "brightwhite" => Color::BrightWhite,
        _ => Color::White, // Default to white
    }
}
fn load_color_rules(wrapper_path: &Path) -> Vec<Rule> {
    let content = fs::read_to_string(wrapper_path).unwrap_or_default();
    let mut rules = Vec::new();
    let mut last_fg = None;
    let mut last_bg = None;
    let mut awaiting_regex = false;

    for (line_num, line) in content.lines().map(str::trim).enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if awaiting_regex {
            match last_fg {
                Some(fg) => match Regex::new(line) {
                    Ok(re) => rules.push(Rule {
                        regex: re,
                        fg_color: fg,
                        bg_color: last_bg,
                    }),
                    Err(err) => {
                        eprintln!("Invalid regex on line {}: {} ({})", line_num + 1, line, err)
                    }
                },
                None => eprintln!(
                    "Regex without preceding color on line {}: {}",
                    line_num + 1,
                    line
                ),
            }
            awaiting_regex = false;
        } else if let Some(color_def) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            let (fg, bg) = parse_colors(color_def);
            if let Some(fg) = fg {
                last_fg = Some(fg);
                last_bg = bg;
                awaiting_regex = true;
            } else {
                eprintln!(
                    "Missing 'fg:' in color definition on line {}: {}",
                    line_num + 1,
                    line
                );
            }
        }
    }

    rules
}

fn parse_colors(color_def: &str) -> (Option<Color>, Option<Color>) {
    let mut fg = None;
    let mut bg = None;

    for part in color_def.split(',').map(str::trim) {
        if let Some(fg_color) = part.strip_prefix("fg:") {
            fg = Some(parse_color(fg_color));
        } else if let Some(bg_color) = part.strip_prefix("bg:") {
            bg = Some(parse_color(bg_color));
        }
    }

    (fg, bg)
}

fn apply_color_rules(line: &str, rules: &[Rule], use_color: bool) -> String {
    if !use_color {
        return line.to_string();
    }

    let mut colored_segments = BTreeMap::new();

    for rule in rules {
        for cap in rule.regex.captures_iter(line) {
            if let Some(matched) = cap.get(0) {
                let start = matched.start();
                let end = matched.end();

                if colored_segments.keys().any(|&(s, e)| start < e && end > s) {
                    continue;
                }

                let mut styled_text = matched.as_str().color(rule.fg_color);

                if let Some(bg) = rule.bg_color {
                    styled_text = styled_text.on_color(bg);
                }

                colored_segments.insert((start, end), styled_text.to_string());
            }
        }
    }

    let mut final_output = String::new();
    let mut last_index = 0;

    for (&(start, end), styled_text) in &colored_segments {
        final_output.push_str(&line[last_index..start]);
        final_output.push_str(styled_text);
        last_index = end;
    }

    if last_index < line.len() {
        final_output.push_str(&line[last_index..]);
    }

    final_output
}

fn main() {
    let wrapped_program = get_wrapped_program().expect("Failed to determine wrapped program");
    let wrapper_path = Path::new(WRAP_DIR).join(&wrapped_program);
    if !wrapper_path.exists() {
        eprintln!("Wrapper script not found: {:?}", wrapper_path);
        std::process::exit(1);
    }

    let real_program = find_real_program(&wrapped_program).unwrap_or_else(|| {
        eprintln!("Could not find real program for '{}'", wrapped_program);
        std::process::exit(1);
    });

    let rules = load_color_rules(&wrapper_path);

    let args: Vec<String> = env::args().skip(2).collect(); // Skipping the wrapper name and program

    let mut child = Command::new(real_program)
        .args(&args)
        .stdout(Stdio::piped())
        .spawn()
        .expect("Failed to spawn real program");

    let stdout = child.stdout.take().expect("Failed to capture stdout");
    let reader = BufReader::new(stdout);
    let stdout_handle = io::stdout();
    let mut out = stdout_handle.lock();
    let use_color = atty::is(Stream::Stdout);

    for line in reader.lines() {
        match line {
            Ok(line) => {
                let colored = apply_color_rules(&line, &rules, use_color);
                writeln!(out, "{}", colored).unwrap();
            }
            Err(e) => {
                eprintln!("Error reading line from child process: {}", e);
                break;
            }
        }
    }

    let _ = child.wait().expect("Failed to wait on child process");
}

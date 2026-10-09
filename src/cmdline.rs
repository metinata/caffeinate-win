//! The utility's arguments, as written on caffeinate's command line.
//!
//! Windows passes a program its command line as one string, which the program
//! splits into arguments itself. Most programs follow the C runtime's rules, as
//! std does, but some read the string their own way, such as `cmd /c`, `find`
//! and `msiexec`. Quoting the split arguments again for the utility could
//! change what these programs see, so caffeinate passes the rest of its own
//! command line on unchanged instead, as `cmd.exe` does when it runs a program.

use std::ffi::OsString;
use std::iter;
use std::os::windows::ffi::OsStringExt;
use std::slice;

use windows_sys::Win32::System::Environment::GetCommandLineW;

const TAB: u16 = b'\t' as u16;
const SPACE: u16 = b' ' as u16;
const QUOTE: u16 = b'"' as u16;
const BACKSLASH: u16 = b'\\' as u16;

/// This process's command line from argument `index` on (0 is the program
/// name), exactly as written. `args` are the arguments std parsed from it;
/// if splitting the command line here does not give the same, returns `None`.
pub fn tail(args: &[OsString], index: usize) -> Option<OsString> {
    // SAFETY: GetCommandLineW returns this process's NUL-terminated command
    // line, which stays valid and unchanged for the life of the process.
    let line = unsafe {
        let start = GetCommandLineW();
        let mut len = 0;
        while *start.add(len) != 0 {
            len += 1;
        }
        slice::from_raw_parts(start, len)
    };
    tail_of(line, args, index)
}

fn tail_of(line: &[u16], args: &[OsString], index: usize) -> Option<OsString> {
    let split = split(line);
    if !split.iter().map(|(_, arg)| arg).eq(args) {
        return None;
    }
    let start = split.get(index).map_or(line.len(), |&(start, _)| start);
    Some(OsString::from_wide(&line[start..]))
}

/// Splits a command line into arguments the way std's `args_os` does, and
/// returns each with the index where it starts.
fn split(line: &[u16]) -> Vec<(usize, OsString)> {
    let mut args = Vec::new();
    if line.is_empty() {
        return args;
    }
    let skip_blanks = |i: &mut usize| {
        while matches!(line.get(*i), Some(&(SPACE | TAB))) {
            *i += 1;
        }
    };

    // The program name ends at the first space or tab outside quotes. Quotes
    // are dropped and nothing is escaped.
    let mut i = 0;
    let mut name = Vec::new();
    let mut in_quotes = false;
    while let Some(&c) = line.get(i) {
        i += 1;
        match c {
            QUOTE => in_quotes = !in_quotes,
            SPACE | TAB if !in_quotes => break,
            _ => name.push(c),
        }
    }
    args.push((0, OsString::from_wide(&name)));
    skip_blanks(&mut i);

    // The arguments, by the C runtime's rules.
    let mut start = i;
    let mut arg = Vec::new();
    let mut in_quotes = false;
    while let Some(&c) = line.get(i) {
        i += 1;
        match c {
            SPACE | TAB if !in_quotes => {
                args.push((start, OsString::from_wide(&arg)));
                arg.clear();
                skip_blanks(&mut i);
                start = i;
            }
            BACKSLASH => {
                let mut count = 1;
                while line.get(i) == Some(&BACKSLASH) {
                    count += 1;
                    i += 1;
                }
                if line.get(i) == Some(&QUOTE) {
                    // Backslashes before a quote escape each other, and an odd
                    // one out escapes the quote.
                    arg.extend(iter::repeat_n(BACKSLASH, count / 2));
                    if count % 2 == 1 {
                        arg.push(QUOTE);
                        i += 1;
                    }
                } else {
                    arg.extend(iter::repeat_n(BACKSLASH, count));
                }
            }
            QUOTE if in_quotes => match line.get(i) {
                // Two quotes inside quotes are a literal quote.
                Some(&QUOTE) => {
                    arg.push(QUOTE);
                    i += 1;
                }
                Some(_) => in_quotes = false,
                // A closing quote at the very end still makes an argument.
                None => break,
            },
            QUOTE => in_quotes = true,
            _ => arg.push(c),
        }
    }
    if !arg.is_empty() || in_quotes {
        args.push((start, OsString::from_wide(&arg)));
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    /// The arguments of `line`, each with the rest of the line from its start.
    fn split_str(line: &str) -> Vec<(String, String)> {
        let line = wide(line);
        split(&line)
            .into_iter()
            .map(|(start, arg)| {
                let rest = String::from_utf16(&line[start..]).unwrap();
                (arg.into_string().unwrap(), rest)
            })
            .collect()
    }

    #[test]
    fn splits_at_spaces_and_tabs() {
        assert_eq!(
            split_str("prog  a\tb "),
            [("prog", "prog  a\tb "), ("a", "a\tb "), ("b", "b ")]
                .map(|(a, r)| (a.to_string(), r.to_string()))
        );
    }

    #[test]
    fn follows_the_quoting_rules() {
        let args = |line: &str| -> Vec<String> {
            split_str(line).into_iter().map(|(arg, _)| arg).collect()
        };
        // The program name keeps backslashes and drops quotes.
        assert_eq!(args(r#""C:\a b\c.exe" x"#), [r"C:\a b\c.exe", "x"]);
        assert_eq!(args(r#"p "a b" "" c"#), ["p", "a b", "", "c"]);
        assert_eq!(args(r#"p a\\\"b c\\"d e""#), ["p", r#"a\"b"#, r"c\d e"]);
        assert_eq!(args(r#"p "a""b" a\\b"#), ["p", r#"a"b"#, r"a\\b"]);
        // Quotes at the end still make an argument, even an empty one.
        assert_eq!(args(r#"p """#), ["p", ""]);
        assert_eq!(args(r#"p "a"#), ["p", "a"]);
        assert_eq!(args("p "), ["p"]);
    }

    #[test]
    fn tail_keeps_the_utility_arguments_as_written() {
        let line = wide(r#"caffeinate -d msiexec /i app.msi DIR="C:\Program Files\App""#);
        let args = [
            "caffeinate",
            "-d",
            "msiexec",
            "/i",
            "app.msi",
            r"DIR=C:\Program Files\App",
        ]
        .map(OsString::from);
        assert_eq!(
            tail_of(&line, &args, 3),
            Some(r#"/i app.msi DIR="C:\Program Files\App""#.into())
        );
        // A utility without arguments.
        let line = wide("caffeinate -d notepad");
        let args = ["caffeinate", "-d", "notepad"].map(OsString::from);
        assert_eq!(tail_of(&line, &args, 3), Some("".into()));
    }

    #[test]
    fn tail_gives_up_when_the_arguments_differ() {
        let line = wide("caffeinate a b");
        let args = ["caffeinate", "a", "c"].map(OsString::from);
        assert_eq!(tail_of(&line, &args, 1), None);
    }
}

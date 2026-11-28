//! What `Bash` reads from a command's text: whether it does nothing but
//! sleep, and whether its exit code 1 is an answer rather than a failure.
//!
//! The reading is shallow on purpose. A command is split at `|`, `&`, `;`,
//! line breaks and parentheses outside quotes, and only the last part counts:
//! in a pipeline the last command decides the exit code.

/// Commands whose exit code 1 is an answer: no match, a difference, a false
/// condition, a folder that could not be read. Only 2 and above is an error.
const EXIT_ONE_ANSWERS: &[&str] = &["grep", "rg", "egrep", "fgrep", "find", "diff", "test", "["];

/// `git` subcommands whose exit code 1 is an answer.
const GIT_EXIT_ONE_ANSWERS: &[&str] = &["grep", "diff"];

/// Words in front of a command that run it rather than name it.
const WRAPPERS: &[&str] = &["time", "command", "builtin", "exec", "env", "nohup", "sudo"];

/// Whether `command` is nothing but a `sleep`, such as `sleep 5` or
/// `sleep 0.5;`. A sleep that is part of a longer command is not.
pub fn is_lone_sleep(command: &str) -> bool {
    let command = command.trim().trim_end_matches([';', ' ', '\t']).trim();
    if command.contains([';', '|', '&', '<', '>', '(', ')', '$', '`', '\n', '{', '}']) {
        return false;
    }
    let mut words = command.split_whitespace();
    let Some(program) = words.next() else {
        return false;
    };
    if program != "sleep" && !program.ends_with("/sleep") {
        return false;
    }
    let mut durations = 0;
    for word in words {
        if !is_duration(word) {
            return false;
        }
        durations += 1;
    }
    durations > 0
}

/// `5`, `0.5`, `10s`, `2m`, `1h`, `1d` or `infinity`.
fn is_duration(word: &str) -> bool {
    if word == "infinity" {
        return true;
    }
    let number = word.strip_suffix(['s', 'm', 'h', 'd']).unwrap_or(word);
    let mut parts = number.splitn(2, '.');
    let whole = parts.next().unwrap_or_default();
    let fraction = parts.next();
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    match fraction {
        None => !whole.is_empty() && digits(whole),
        Some(fraction) => {
            (!whole.is_empty() || !fraction.is_empty()) && digits(whole) && digits(fraction)
        }
    }
}

/// Whether the exit code `code` of `command` is an error. Every code but 0
/// is, except 1 for a search or comparison command (`grep`, `rg`, `egrep`,
/// `fgrep`, `find`, `diff`, `test`, `[`, `git grep`, `git diff`) as the last
/// command.
pub fn is_error_exit(command: &str, code: i32) -> bool {
    match code {
        0 => false,
        1 => !exit_one_is_an_answer(command),
        _ => true,
    }
}

fn exit_one_is_an_answer(command: &str) -> bool {
    let Some(words) = last_command(command) else {
        return false;
    };
    let mut words = words
        .into_iter()
        .skip_while(|word| is_assignment(word) || WRAPPERS.contains(&word.as_str()) || word == "!");
    let Some(program) = words.next() else {
        return false;
    };
    let program = program.rsplit(['/', '\\']).next().unwrap_or(&program);
    let program = program.strip_suffix(".exe").unwrap_or(program);
    if program == "git" {
        let mut words = words.peekable();
        while let Some(word) = words.next() {
            if word == "-C" || word == "-c" {
                words.next();
            } else if !word.starts_with('-') {
                return GIT_EXIT_ONE_ANSWERS.contains(&word.as_str());
            }
        }
        return false;
    }
    EXIT_ONE_ANSWERS.contains(&program)
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            && !name.as_bytes()[0].is_ascii_digit()
    })
}

/// The words of the last command in `command`, unquoted, or `None` when it
/// has none. `{` and `}` of a group are left out.
fn last_command(command: &str) -> Option<Vec<String>> {
    let mut commands: Vec<Vec<String>> = vec![Vec::new()];
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let characters: Vec<char> = command.chars().collect();
    let mut index = 0;
    let end_word = |word: &mut String, in_word: &mut bool, commands: &mut Vec<Vec<String>>| {
        if *in_word {
            let done = std::mem::take(word);
            if done != "{" && done != "}" {
                commands.last_mut().unwrap().push(done);
            }
            *in_word = false;
        }
    };
    while index < characters.len() {
        let character = characters[index];
        match quote {
            Some('\'') => {
                if character == '\'' {
                    quote = None;
                } else {
                    word.push(character);
                }
            }
            Some(_) => {
                if character == '"' {
                    quote = None;
                } else if character == '\\' && index + 1 < characters.len() {
                    index += 1;
                    word.push(characters[index]);
                } else {
                    word.push(character);
                }
            }
            None => match character {
                '\'' | '"' => {
                    quote = Some(character);
                    in_word = true;
                }
                '\\' if index + 1 < characters.len() => {
                    index += 1;
                    if characters[index] != '\n' {
                        word.push(characters[index]);
                        in_word = true;
                    }
                }
                // `2>&1`, `>&2` and `&>` redirect; they do not end a command.
                '&' if (index > 0 && matches!(characters[index - 1], '>' | '<'))
                    || characters.get(index + 1) == Some(&'>') =>
                {
                    word.push(character);
                    in_word = true;
                }
                '|' | '&' | ';' | '\n' | '(' | ')' => {
                    end_word(&mut word, &mut in_word, &mut commands);
                    if !commands.last().unwrap().is_empty() {
                        commands.push(Vec::new());
                    }
                }
                character if character.is_whitespace() => {
                    end_word(&mut word, &mut in_word, &mut commands);
                }
                character => {
                    word.push(character);
                    in_word = true;
                }
            },
        }
        index += 1;
    }
    end_word(&mut word, &mut in_word, &mut commands);
    commands.into_iter().rev().find(|words| !words.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_lone_sleep_is_one() {
        for command in [
            "sleep 5",
            "  sleep 0.5 ",
            "sleep 10s;",
            "sleep 1m 30s",
            "/bin/sleep 2",
            "sleep .5",
            "sleep infinity",
        ] {
            assert!(is_lone_sleep(command), "{command}");
        }
        for command in [
            "sleep",
            "sleep 5 && echo done",
            "sleep 5; echo done",
            "echo start; sleep 5",
            "sleep 5 &",
            "sleep $DELAY",
            "sleep five",
            "sleeper 5",
            "sleep 5 | cat",
            "(sleep 5)",
            "sleep 1\nsleep 2",
        ] {
            assert!(!is_lone_sleep(command), "{command}");
        }
    }

    #[test]
    fn exit_code_one_is_an_answer_for_search_and_comparison_commands() {
        for command in [
            "grep needle haystack.txt",
            "rg needle",
            "egrep 'a|b' file",
            "fgrep x file",
            "find . -name '*.rs'",
            "diff a b",
            "test -f missing",
            "[ -f missing ]",
            "git grep needle",
            "git diff --exit-code",
            "git -C repo --no-pager diff --quiet",
            "cat file | grep needle",
            "cd src && grep -r needle .",
            "LC_ALL=C grep needle file",
            "/usr/bin/grep needle file",
            "grep needle file 2>&1",
            "grep needle file &> /dev/null",
            "time grep needle file",
            "! grep needle file",
            "{ grep needle file; }",
            "(cd src && grep needle file)",
            "echo 'a | b' | grep \"x;y\"",
        ] {
            assert!(!is_error_exit(command, 1), "{command}");
            assert!(is_error_exit(command, 2), "{command}");
            assert!(!is_error_exit(command, 0), "{command}");
        }
    }

    #[test]
    fn the_last_command_decides() {
        for command in [
            "false",
            "grep needle file | wc -l",
            "grep needle file && echo found",
            "grep needle file; exit 1",
            "git status",
            "git commit -m diff",
            "echo grep",
            "cargo test",
            "exit 1",
        ] {
            assert!(is_error_exit(command, 1), "{command}");
        }
    }
}

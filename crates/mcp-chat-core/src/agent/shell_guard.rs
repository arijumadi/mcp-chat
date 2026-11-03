//! Detects shell commands that write file contents directly, so `bash` can be
//! kept for build/test/git/system work and edits go through `write`,
//! `apply_patch` or `edit`.
//!
//! This is a routing aid, not a security boundary. It is deliberately
//! conservative: it looks only at the shell's own syntax (redirects, `tee`,
//! `sed -i`, `dd of=`, and the PowerShell file cmdlets) and ignores quoted
//! text, heredoc bodies and comments. Build tools that write generated files,
//! and file management such as `mkdir`, `rm` and `mv`, are never flagged.

/// How many levels of `sh -c "..."` payloads are inspected.
const MAX_NESTING: usize = 3;

/// True when the command clearly writes text into a file.
pub fn writes_file_contents(command: &str) -> bool {
    scan(command, 0)
}

#[derive(Debug)]
struct Token {
    /// The token with quotes and escapes removed.
    text: String,
    /// Whether any part of the token was quoted.
    quoted: bool,
}

fn scan(command: &str, depth: usize) -> bool {
    let source = strip_here_documents(command);
    let Some(segments) = tokenize(&source) else {
        // A redirect to a real file was found while tokenizing.
        return true;
    };
    segments
        .iter()
        .any(|segment| segment_writes(segment, depth))
}

/// Drop heredoc and PowerShell here-string bodies. Their content is data, and
/// it often contains `>` characters that are not redirects.
fn strip_here_documents(source: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut pending: Vec<(String, bool)> = Vec::new();
    let mut here_string: Option<char> = None;
    for line in source.lines() {
        if let Some(quote) = here_string {
            if line.trim_start().starts_with(&format!("{quote}@")) {
                here_string = None;
            }
            continue;
        }
        if let Some((delimiter, strip_tabs)) = pending.first() {
            let candidate = if *strip_tabs {
                line.trim_start_matches('\t')
            } else {
                line
            };
            if candidate.trim_end() == delimiter.as_str() {
                pending.remove(0);
            }
            continue;
        }
        out.push(line);
        pending.extend(heredoc_delimiters(line));
        let trimmed = line.trim_end();
        if trimmed.ends_with("@'") {
            here_string = Some('\'');
        } else if trimmed.ends_with("@\"") {
            here_string = Some('"');
        }
    }
    out.join("\n")
}

/// Delimiters introduced by `<<WORD`, `<<-WORD`, `<<'WORD'` or `<<"WORD"` on one line.
fn heredoc_delimiters(line: &str) -> Vec<(String, bool)> {
    let chars: Vec<char> = line.chars().collect();
    let mut found = Vec::new();
    let mut index = 0;
    while index + 1 < chars.len() {
        if chars[index] == '<' && chars[index + 1] == '<' {
            // `<<<` is a here-string, not a heredoc.
            if chars.get(index + 2) == Some(&'<') {
                index += 3;
                continue;
            }
            let mut cursor = index + 2;
            let strip_tabs = chars.get(cursor) == Some(&'-');
            if strip_tabs {
                cursor += 1;
            }
            while chars.get(cursor).is_some_and(|c| *c == ' ' || *c == '\t') {
                cursor += 1;
            }
            let quote = chars
                .get(cursor)
                .copied()
                .filter(|c| *c == '\'' || *c == '"');
            if quote.is_some() {
                cursor += 1;
            }
            let start = cursor;
            while chars
                .get(cursor)
                .is_some_and(|c| c.is_alphanumeric() || *c == '_')
            {
                cursor += 1;
            }
            let word: String = chars[start..cursor].iter().collect();
            // A leading digit means arithmetic such as `1<<2`, not a heredoc.
            if !word.is_empty() && !word.starts_with(|c: char| c.is_ascii_digit()) {
                found.push((word, strip_tabs));
            }
            index = cursor.max(index + 2);
        } else {
            index += 1;
        }
    }
    found
}

/// Split a command into segments of tokens. Returns `None` as soon as a
/// redirect to a real file is seen.
fn tokenize(source: &str) -> Option<Vec<Vec<Token>>> {
    let chars: Vec<char> = source.chars().collect();
    let mut segments: Vec<Vec<Token>> = Vec::new();
    let mut tokens: Vec<Token> = Vec::new();
    let mut text = String::new();
    let mut quoted = false;
    let mut in_token = false;
    let mut index = 0;

    macro_rules! end_token {
        () => {
            if in_token {
                tokens.push(Token {
                    text: std::mem::take(&mut text),
                    quoted,
                });
                quoted = false;
                in_token = false;
            }
        };
    }
    macro_rules! end_segment {
        () => {
            end_token!();
            if !tokens.is_empty() {
                segments.push(std::mem::take(&mut tokens));
            }
        };
    }

    while index < chars.len() {
        let c = chars[index];
        match c {
            ' ' | '\t' | '\r' => {
                end_token!();
                index += 1;
            }
            '\n' | ';' | '|' | '(' | ')' | '{' | '}' => {
                // `$(( ... ))` is arithmetic: its `>` is a comparison.
                if c == '(' && text == "$" && chars.get(index + 1) == Some(&'(') {
                    index = skip_arithmetic(&chars, index);
                    continue;
                }
                end_segment!();
                index += 1;
            }
            '&' => {
                // `&>file` is a redirect; `&&`, `&` and `>&` are separators or fd copies.
                if chars.get(index + 1) == Some(&'>') {
                    end_token!();
                    if redirect_writes_file(&chars, index + 1) {
                        return None;
                    }
                    index += 2;
                    if chars.get(index) == Some(&'>') {
                        index += 1;
                    }
                    continue;
                }
                end_segment!();
                index += 1;
            }
            '#' if !in_token => {
                // A comment runs to the end of the line.
                while index < chars.len() && chars[index] != '\n' {
                    index += 1;
                }
            }
            '>' => {
                // Comparisons and arrows such as `>=`, `->` and `=>` are not redirects.
                let previous = index.checked_sub(1).map(|i| chars[i]);
                let next = chars.get(index + 1).copied();
                if next == Some('=') || matches!(previous, Some('-') | Some('=')) {
                    text.push(c);
                    in_token = true;
                    index += 1;
                    continue;
                }
                // A leading fd number (`2>`) belongs to the redirect, not to a word.
                if in_token
                    && !quoted
                    && !text.is_empty()
                    && text.chars().all(|ch| ch.is_ascii_digit() || ch == '*')
                {
                    text.clear();
                    in_token = false;
                }
                end_token!();
                if redirect_writes_file(&chars, index) {
                    return None;
                }
                index = skip_redirect_operator(&chars, index);
            }
            '\\' | '`' => {
                // Escaped characters are literal. A backtick is the PowerShell escape;
                // a backslash is the POSIX one and is left alone before ordinary characters.
                match chars.get(index + 1) {
                    Some(&next) if is_escapable(c, next) => {
                        text.push(next);
                        in_token = true;
                        index += 2;
                    }
                    _ => {
                        text.push(c);
                        in_token = true;
                        index += 1;
                    }
                }
            }
            '\'' => {
                in_token = true;
                quoted = true;
                index += 1;
                while index < chars.len() && chars[index] != '\'' {
                    text.push(chars[index]);
                    index += 1;
                }
                index += 1;
            }
            '"' => {
                in_token = true;
                quoted = true;
                index += 1;
                while index < chars.len() && chars[index] != '"' {
                    if matches!(chars[index], '\\' | '`')
                        && chars
                            .get(index + 1)
                            .is_some_and(|next| matches!(next, '"' | '\\' | '$' | '`'))
                    {
                        index += 1;
                    }
                    text.push(chars[index]);
                    index += 1;
                }
                index += 1;
            }
            _ => {
                text.push(c);
                in_token = true;
                index += 1;
            }
        }
    }
    if in_token {
        tokens.push(Token { text, quoted });
    }
    if !tokens.is_empty() {
        segments.push(tokens);
    }
    Some(segments)
}

fn is_escapable(escape: char, next: char) -> bool {
    match escape {
        '`' => matches!(next, '>' | '"' | '\'' | '$' | '`' | '|' | ';' | '&'),
        _ => matches!(
            next,
            '>' | '<' | '"' | '\'' | '|' | ';' | '&' | '$' | ' ' | '\\' | '#'
        ),
    }
}

/// Index just after the `))` that closes a `$((` expression.
fn skip_arithmetic(chars: &[char], open: usize) -> usize {
    let mut depth = 0usize;
    let mut index = open;
    while index < chars.len() {
        match chars[index] {
            '(' => depth += 1,
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return index + 1;
                }
            }
            _ => {}
        }
        index += 1;
    }
    chars.len()
}

/// Index after `>`, `>>`, `>|` or `>&` starting at the first `>`.
fn skip_redirect_operator(chars: &[char], start: usize) -> usize {
    let mut index = start + 1;
    if chars.get(index) == Some(&'>') {
        index += 1;
    }
    if matches!(chars.get(index), Some('|') | Some('&')) {
        index += 1;
    }
    index
}

/// Whether the redirect whose first `>` is at `start` targets a real file.
fn redirect_writes_file(chars: &[char], start: usize) -> bool {
    let mut index = start + 1;
    if chars.get(index) == Some(&'>') {
        index += 1;
    }
    match chars.get(index) {
        Some('|') => index += 1,
        // `>&1`, `>&2` and `>&-` copy or close a descriptor.
        Some('&') => {
            index += 1;
            if chars
                .get(index)
                .is_some_and(|c| c.is_ascii_digit() || *c == '-')
            {
                return false;
            }
        }
        _ => {}
    }
    while chars.get(index).is_some_and(|c| *c == ' ' || *c == '\t') {
        index += 1;
    }
    let mut target = String::new();
    match chars.get(index).copied() {
        Some(quote @ ('\'' | '"')) => {
            index += 1;
            while index < chars.len() && chars[index] != quote {
                target.push(chars[index]);
                index += 1;
            }
        }
        _ => {
            while let Some(&c) = chars.get(index) {
                if c.is_whitespace() || matches!(c, ';' | '|' | '&' | '<' | '>' | '(' | ')') {
                    break;
                }
                target.push(c);
                index += 1;
            }
        }
    }
    !is_discard_target(&target)
}

/// Targets that are not files: an empty target, the null device or a standard stream.
fn is_discard_target(target: &str) -> bool {
    let target = target.trim().to_ascii_lowercase();
    target.is_empty()
        || matches!(
            target.as_str(),
            "/dev/null" | "/dev/stdout" | "/dev/stderr" | "/dev/tty" | "nul" | "nul:" | "$null"
        )
        || target.starts_with("/dev/fd/")
}

const WRAPPERS: &[&str] = &[
    "sudo", "doas", "nohup", "time", "exec", "command", "builtin", "env", "xargs", "nice",
];
const SHELLS: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "dash",
    "ash",
    "ksh",
    "pwsh",
    "powershell",
    "cmd",
];
const POWERSHELL_WRITERS: &[&str] = &[
    "set-content",
    "add-content",
    "out-file",
    "clear-content",
    "tee-object",
];

fn command_name(token: &Token) -> String {
    if token.quoted {
        return String::new();
    }
    let lowered = token.text.to_ascii_lowercase();
    let base = lowered.rsplit(['/', '\\']).next().unwrap_or("");
    base.strip_suffix(".exe").unwrap_or(base).to_owned()
}

fn is_env_assignment(token: &Token) -> bool {
    if token.quoted {
        return false;
    }
    match token.text.split_once('=') {
        Some((name, _)) => {
            !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !name.starts_with(|c: char| c.is_ascii_digit())
        }
        None => false,
    }
}

fn segment_writes(tokens: &[Token], depth: usize) -> bool {
    if command_writes(tokens, 0, depth) {
        return true;
    }
    // `$x = [IO.File]::WriteAllText(...)` is not at command position.
    if tokens
        .iter()
        .any(|token| !token.quoted && dotnet_file_write(&token.text.to_ascii_lowercase()))
    {
        return true;
    }
    // `find ... -exec sed -i ... {} \;` runs another command.
    tokens.iter().enumerate().any(|(index, token)| {
        !token.quoted
            && matches!(token.text.as_str(), "-exec" | "-execdir" | "-ok" | "-okdir")
            && command_writes(tokens, index + 1, depth)
    })
}

/// Inspect the command that starts at `start`, looking through wrappers such
/// as `sudo` and `xargs`.
fn command_writes(tokens: &[Token], start: usize, depth: usize) -> bool {
    let mut index = start;
    loop {
        while tokens.get(index).is_some_and(is_env_assignment) {
            index += 1;
        }
        let Some(token) = tokens.get(index) else {
            return false;
        };
        let name = command_name(token);
        if WRAPPERS.contains(&name.as_str()) {
            index += 1;
            while tokens
                .get(index)
                .is_some_and(|t| !t.quoted && t.text.starts_with('-'))
            {
                index += 1;
            }
            continue;
        }
        return named_command_writes(&name, &tokens[index + 1..], depth);
    }
}

fn named_command_writes(name: &str, args: &[Token], depth: usize) -> bool {
    match name {
        "tee" => args
            .iter()
            .any(|arg| !arg.text.starts_with('-') && !is_discard_target(&arg.text)),
        "sed" => args.iter().any(|arg| {
            arg.text == "--in-place"
                || arg.text.starts_with("--in-place=")
                || in_place_flag(&arg.text, "nErszu")
        }),
        "perl" => args
            .iter()
            .any(|arg| in_place_flag(&arg.text, "nplaWwStTUuxX")),
        "dd" => args.iter().any(|arg| {
            arg.text
                .strip_prefix("of=")
                .is_some_and(|target| !is_discard_target(target))
        }),
        "new-item" | "ni" => creates_file(args),
        name if POWERSHELL_WRITERS.contains(&name) => true,
        name if SHELLS.contains(&name) => nested_shell_writes(args, depth),
        _ => false,
    }
}

/// `-i`, `-i.bak`, `-ni` and similar: an in-place switch inside a flag cluster.
/// `leading` lists the switches that take no argument and may precede `i`.
fn in_place_flag(text: &str, leading: &str) -> bool {
    let Some(rest) = text.strip_prefix('-') else {
        return false;
    };
    if rest.starts_with('-') {
        return false;
    }
    for c in rest.chars() {
        if c == 'i' {
            return true;
        }
        if !leading.contains(c) {
            return false;
        }
    }
    false
}

/// `New-Item -ItemType File` (also `-Type file`, `-ItemType:File`).
fn creates_file(args: &[Token]) -> bool {
    for (index, arg) in args.iter().enumerate() {
        if arg.quoted {
            continue;
        }
        let lowered = arg.text.to_ascii_lowercase();
        let Some(flag) = lowered.strip_prefix('-') else {
            continue;
        };
        let (name, inline) = match flag.split_once(':') {
            Some((name, value)) => (name, Some(value.to_owned())),
            None => (flag, None),
        };
        let is_type =
            !name.is_empty() && ("itemtype".starts_with(name) || "type".starts_with(name));
        if !is_type {
            continue;
        }
        let value = inline.or_else(|| args.get(index + 1).map(|t| t.text.to_ascii_lowercase()));
        if value.is_some_and(|v| v == "file") {
            return true;
        }
    }
    false
}

/// `[System.IO.File]::WriteAllText(...)`, `[IO.File]::AppendAllLines(...)` and friends.
/// Brackets and `::` are not separators, so the call name is one lowercase token.
fn dotnet_file_write(token: &str) -> bool {
    let Some(rest) = token
        .strip_prefix("[system.io.file]::")
        .or_else(|| token.strip_prefix("[io.file]::"))
    else {
        return false;
    };
    rest.starts_with("write") || rest.starts_with("append")
}

/// `bash -c "..."`, `powershell -Command ...`, `cmd /c ...`: inspect the payload.
fn nested_shell_writes(args: &[Token], depth: usize) -> bool {
    if depth + 1 >= MAX_NESTING {
        return false;
    }
    for (index, arg) in args.iter().enumerate() {
        if arg.quoted {
            continue;
        }
        let flag = arg.text.to_ascii_lowercase();
        let runs_command = flag == "/c"
            || flag == "/k"
            || flag == "-command"
            || (flag.starts_with('-')
                && !flag.starts_with("--")
                && flag.ends_with('c')
                && flag.len() <= 4);
        if runs_command {
            let payload = args[index + 1..]
                .iter()
                .map(|token| token.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            return scan(&payload, depth + 1);
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocked(command: &str) {
        assert!(
            writes_file_contents(command),
            "expected to block: {command}"
        );
    }

    fn allowed(command: &str) {
        assert!(
            !writes_file_contents(command),
            "expected to allow: {command}"
        );
    }

    #[test]
    fn blocks_powershell_file_writers() {
        blocked("Set-Content -Path a.txt -Value hi");
        blocked("set-content a.txt 'hi'");
        blocked("Add-Content a.txt 'more'");
        blocked("'hi' | Out-File a.txt");
        blocked("Clear-Content a.txt");
        blocked("Get-Date | Tee-Object -FilePath out.txt");
        blocked("Get-ChildItem | ForEach-Object { Set-Content $_.Name 'x' }");
        blocked("[System.IO.File]::WriteAllText('a.txt', 'hi')");
        blocked("[IO.File]::AppendAllText('a.txt', 'hi')");
        blocked("New-Item -ItemType File -Path a.txt");
        blocked("New-Item a.txt -Type file");
        blocked("New-Item a.txt -ItemType:File");
    }

    #[test]
    fn blocks_shell_redirects_and_writers() {
        blocked("echo hi > a.txt");
        blocked("echo hi >> a.txt");
        blocked("echo hi >a.txt");
        blocked("echo hi >| a.txt");
        blocked("printf 'x' > a.txt");
        blocked("cat > a.txt <<'EOF'\nbody\nEOF");
        blocked("echo hi 1> a.txt");
        blocked("ls 2> errors.log");
        blocked("echo hi &> a.txt");
        blocked("echo hi > \"my file.txt\"");
        blocked("npm run build > build.log");
        blocked("echo hi | tee a.txt");
        blocked("echo hi | tee -a a.txt");
        blocked("echo hi | sudo tee /etc/hosts");
        blocked("sed -i 's/a/b/' f.txt");
        blocked("sed -i.bak 's/a/b/' f.txt");
        blocked("sed -ni 's/a/b/p' f.txt");
        blocked("sed 's/a/b/' --in-place f.txt");
        blocked("find . -name '*.js' | xargs sed -i 's/a/b/'");
        blocked("find . -type f -exec sed -i 's/a/b/' {} \\;");
        blocked("dd if=/dev/zero of=out.bin count=1");
        blocked("perl -pi -e 's/a/b/' f.txt");
        blocked("echo done; echo hi > a.txt");
        blocked("cd src && echo hi > a.txt");
    }

    #[test]
    fn blocks_writes_hidden_inside_a_nested_shell() {
        blocked("bash -c \"echo hi > a.txt\"");
        blocked("sh -lc 'echo hi > a.txt'");
        blocked("powershell -Command \"Set-Content a.txt hi\"");
        blocked("pwsh -c Set-Content a.txt hi");
        blocked("cmd /c \"echo hi > a.txt\"");
        allowed("bash -c \"echo hi\"");
        allowed("powershell -Command \"Get-ChildItem\"");
    }

    #[test]
    fn allows_build_test_git_and_package_managers() {
        for command in [
            "npm test",
            "npm run build",
            "pnpm check",
            "pnpm tauri build",
            "pnpm install --frozen-lockfile",
            "cargo test -p mcp-chat-core",
            "cargo build --release 2>&1",
            "cargo test 2>&1 | tail -20",
            "git status",
            "git diff --stat",
            "git log --oneline -5",
            "git commit -m \"fix: write > read\"",
            "pip install requests",
            "docker run --rm -v $(pwd):/app image",
            "curl -o out.bin https://example.com/file",
        ] {
            allowed(command);
        }
    }

    #[test]
    fn allows_file_management_that_does_not_write_contents() {
        for command in [
            "mkdir -p a/b",
            "rm -rf build",
            "mv a.txt b.txt",
            "cp a.txt b.txt",
            "touch a.txt",
            "Move-Item a.txt b.txt",
            "Rename-Item a.txt b.txt",
            "Copy-Item a.txt b.txt",
            "Remove-Item -Recurse build",
            "New-Item -ItemType Directory build",
            "New-Item -ItemType Directory -Path build",
            "mkdir build",
            "ls -la",
        ] {
            allowed(command);
        }
    }

    #[test]
    fn allows_redirects_that_do_not_target_a_file() {
        allowed("ls > /dev/null");
        allowed("cmd 2>/dev/null");
        allowed("cmd >/dev/null 2>&1");
        allowed("cmd 2>&1");
        allowed("cmd 1>&2");
        allowed("dir >nul");
        allowed("dir > NUL 2>&1");
        allowed("Get-Content a.txt > $null");
        allowed("Get-Content a.txt *> $null");
        allowed("echo hi | tee");
        allowed("dd if=a.bin of=/dev/null");
    }

    #[test]
    fn ignores_operators_inside_quotes_comments_and_heredoc_bodies() {
        allowed("echo \"a > b\"");
        allowed("echo 'a >> b'");
        allowed("grep -rn \"foo > bar\" .");
        allowed("echo hi # > out.txt");
        allowed("python - <<'EOF'\nprint(1 > 0)\nEOF");
        allowed("cat <<EOF\nline > not a redirect\nEOF");
        allowed("Write-Output @'\nset-content in text\n'@");
        allowed("echo $((3>2))");
        allowed("echo $PATH");
        allowed("node -e \"console.log(1 > 0)\"");
        allowed("Get-ChildItem | Where-Object { $_.Length -gt 100 }");
        allowed("echo a=>b");
        allowed("echo a->b");
    }

    #[test]
    fn a_heredoc_does_not_hide_a_redirect_on_its_own_line() {
        blocked("python - <<'EOF' > out.txt\nprint('x')\nEOF");
        blocked("cat <<EOF > out.txt\nhello\nEOF");
    }

    #[test]
    fn only_flags_commands_that_really_are_those_commands() {
        allowed("sed -n '1,5p' f.txt");
        allowed("sed 's/a/b/' f.txt");
        allowed("sed -e 's/i/x/' f.txt");
        allowed("sed -E 's/(a)/\\1/' f.txt");
        allowed("perl -MList::Util -e 'print 1'");
        allowed("perl -e 'print 1'");
        allowed("grep -n tee notes.txt");
        allowed("man tee");
        allowed("echo tee a.txt");
        allowed("echo Set-Content");
        allowed("which sed");
        allowed("git commit -m \"use sed -i\"");
    }

    #[test]
    fn detects_nothing_in_an_empty_command() {
        allowed("");
        allowed("   ");
        allowed("\n\n");
    }
}

/// Command history management for terminal input.
///
/// Loads history from shell-specific histfiles on startup (zsh, bash, fish, nushell),
/// tracks commands during the session, and provides frecency-scored search
/// for ghost-text suggestions and history panel filtering.
use std::collections::HashMap;
use std::io::BufRead;
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use inazuma::{App, Global};

/// A single history entry with metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub command: String,
    pub timestamp: u64,
    pub frequency: u32,
    pub exit_status: Option<i32>,
    pub cwd: Option<String>,
}

impl HistoryEntry {
    /// Build an entry from a command string alone. Timestamp is set
    /// to 0 (caller can override), frequency 1, exit/cwd empty.
    /// Ergonomic helper for tests and programmatic history building.
    pub fn from_command(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            timestamp: 0,
            frequency: 1,
            exit_status: None,
            cwd: None,
        }
    }
}

/// One scored match result from a fuzzy query over history.
#[derive(Debug, Clone, PartialEq)]
pub struct FuzzyMatch<'a> {
    pub entry: &'a HistoryEntry,
    pub score: f64,
    pub positions: Vec<usize>,
}

/// Shell-specific history file format.
#[derive(Debug, Clone, Copy)]
pub enum HistfileFormat {
    /// zsh: `: timestamp:duration;command`
    Zsh,
    /// bash: one command per line (optionally with `#timestamp` lines)
    Bash,
    /// fish: YAML-like `- cmd: command\n  when: timestamp`
    Fish,
    /// nushell: plaintext, one command per line
    NuPlaintext,
}

/// Command history store with frecency search.
///
/// Navigation (Up/Down browsing) is handled by `HistoryPanel` which copies
/// entries and manages its own selection state.
pub struct CommandHistory {
    entries: Vec<HistoryEntry>,
    dedup_index: HashMap<String, usize>,
}

impl CommandHistory {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            dedup_index: HashMap::new(),
        }
    }

    /// Auto-detect and load history for the given shell.
    pub fn detect_and_load(shell_name: &str) -> Self {
        let result = match shell_name {
            "zsh" => {
                let path = dirs::home_dir().map(|h| h.join(".zsh_history"));
                path.and_then(|p| Self::load_from_histfile(&p, HistfileFormat::Zsh).ok())
            }
            "bash" => {
                let path = dirs::home_dir().map(|h| h.join(".bash_history"));
                path.and_then(|p| Self::load_from_histfile(&p, HistfileFormat::Bash).ok())
            }
            "fish" => {
                let path = dirs::data_local_dir()
                    .or_else(dirs::config_dir)
                    .map(|c| c.join("fish/fish_history"));
                path.and_then(|p| Self::load_from_histfile(&p, HistfileFormat::Fish).ok())
            }
            "nu" => {
                // Try SQLite first, then plaintext
                let sqlite_path = dirs::config_dir().map(|c| c.join("nushell/history.sqlite3"));
                if let Some(ref p) = sqlite_path {
                    if p.exists() {
                        return Self::load_nu_sqlite(p).unwrap_or_else(|_| Self::new());
                    }
                }
                let txt_path = dirs::config_dir().map(|c| c.join("nushell/history.txt"));
                txt_path
                    .and_then(|p| Self::load_from_histfile(&p, HistfileFormat::NuPlaintext).ok())
            }
            _ => None,
        };
        result.unwrap_or_else(Self::new)
    }

    /// Load history from a text-based histfile.
    pub fn load_from_histfile(path: &Path, format: HistfileFormat) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::new());
        }
        let content = std::fs::read(path)?;
        let mut history = Self::new();

        match format {
            HistfileFormat::Zsh => history.parse_zsh(&content),
            HistfileFormat::Bash => history.parse_bash(&content),
            HistfileFormat::Fish => history.parse_fish(&content),
            HistfileFormat::NuPlaintext => history.parse_plaintext(&content),
        }

        Ok(history)
    }

    /// Load nushell SQLite history.
    #[cfg(feature = "nushell-history")]
    fn load_nu_sqlite(path: &Path) -> Result<Self> {
        use rusqlite::Connection;
        let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mut stmt = conn.prepare(
            "SELECT command_line, start_timestamp, exit_status, cwd
             FROM history ORDER BY start_timestamp ASC LIMIT 10000",
        )?;
        let mut history = Self::new();
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1).unwrap_or(0),
                row.get::<_, Option<i32>>(2).unwrap_or(None),
                row.get::<_, Option<String>>(3).unwrap_or(None),
            ))
        })?;
        for row in rows {
            if let Ok((cmd, ts, exit_status, cwd)) = row {
                let cmd = cmd.trim().to_string();
                if cmd.is_empty() {
                    continue;
                }
                history.insert_entry(HistoryEntry {
                    command: cmd,
                    timestamp: ts as u64,
                    frequency: 1,
                    exit_status,
                    cwd,
                });
            }
        }
        Ok(history)
    }

    #[cfg(not(feature = "nushell-history"))]
    fn load_nu_sqlite(_path: &Path) -> Result<Self> {
        Ok(Self::new())
    }

    // --- Parsers ---

    fn parse_zsh(&mut self, content: &[u8]) {
        // zsh extended history format: `: timestamp:duration;command`
        // Multi-line commands: lines ending with `\` are continuations.
        // Lines without `: ` prefix after a `\`-terminated line are continuations.
        let text = String::from_utf8_lossy(content);
        let mut pending_cmd: Option<String> = None;
        let mut pending_ts: u64 = 0;

        for line in text.lines() {
            if line.starts_with(": ") {
                // Flush previous pending multi-line command
                if let Some(cmd) = pending_cmd.take() {
                    if !cmd.is_empty() {
                        self.insert_entry(HistoryEntry {
                            command: cmd,
                            timestamp: pending_ts,
                            frequency: 1,
                            exit_status: None,
                            cwd: None,
                        });
                    }
                }

                // Parse new entry: `: timestamp:duration;command`
                if let Some(semi_pos) = line.find(';') {
                    let meta = &line[2..semi_pos];
                    let command_part = &line[semi_pos + 1..];
                    pending_ts = meta
                        .split(':')
                        .next()
                        .and_then(|s| s.trim().parse::<u64>().ok())
                        .unwrap_or(0);

                    if let Some(stripped) = command_part.strip_suffix('\\') {
                        // Multi-line command starts — strip trailing backslash
                        pending_cmd = Some(stripped.to_string());
                    } else if !command_part.is_empty() {
                        self.insert_entry(HistoryEntry {
                            command: command_part.to_string(),
                            timestamp: pending_ts,
                            frequency: 1,
                            exit_status: None,
                            cwd: None,
                        });
                    }
                }
            } else if let Some(ref mut cmd) = pending_cmd {
                // Continuation line of a multi-line command
                cmd.push('\n');
                if let Some(stripped) = line.strip_suffix('\\') {
                    cmd.push_str(stripped);
                } else {
                    cmd.push_str(line);
                    // Multi-line command complete
                    let finished = std::mem::take(cmd);
                    if !finished.is_empty() {
                        self.insert_entry(HistoryEntry {
                            command: finished,
                            timestamp: pending_ts,
                            frequency: 1,
                            exit_status: None,
                            cwd: None,
                        });
                    }
                    pending_cmd = None;
                }
            }
            // Lines without `: ` prefix and no pending continuation are ignored
        }

        // Flush last pending command
        if let Some(cmd) = pending_cmd {
            if !cmd.is_empty() {
                self.insert_entry(HistoryEntry {
                    command: cmd,
                    timestamp: pending_ts,
                    frequency: 1,
                    exit_status: None,
                    cwd: None,
                });
            }
        }
    }

    fn parse_bash(&mut self, content: &[u8]) {
        let reader = std::io::BufReader::new(content);
        let mut pending_timestamp: Option<u64> = None;
        for line in reader.lines().map_while(Result::ok) {
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }
            if let Some(ts_str) = line.strip_prefix('#') {
                // Timestamp line: #1234567890
                pending_timestamp = ts_str.trim().parse::<u64>().ok();
                continue;
            }
            self.insert_entry(HistoryEntry {
                command: line,
                timestamp: pending_timestamp.take().unwrap_or(0),
                frequency: 1,
                exit_status: None,
                cwd: None,
            });
        }
    }

    fn parse_fish(&mut self, content: &[u8]) {
        // fish format:
        // - cmd: some command
        //   when: 1234567890
        //   paths:
        //     - /some/path
        let text = String::from_utf8_lossy(content);
        let mut current_cmd: Option<String> = None;
        let mut current_ts: u64 = 0;

        for line in text.lines() {
            if let Some(cmd_str) = line.strip_prefix("- cmd: ") {
                // Save previous entry
                if let Some(cmd) = current_cmd.take() {
                    if !cmd.is_empty() {
                        self.insert_entry(HistoryEntry {
                            command: cmd,
                            timestamp: current_ts,
                            frequency: 1,
                            exit_status: None,
                            cwd: None,
                        });
                    }
                }
                current_cmd = Some(cmd_str.to_string());
                current_ts = 0;
            } else if let Some(when_str) = line.trim_start().strip_prefix("when: ") {
                current_ts = when_str.trim().parse().unwrap_or(0);
            }
        }
        // Don't forget the last entry
        if let Some(cmd) = current_cmd {
            if !cmd.is_empty() {
                self.insert_entry(HistoryEntry {
                    command: cmd,
                    timestamp: current_ts,
                    frequency: 1,
                    exit_status: None,
                    cwd: None,
                });
            }
        }
    }

    fn parse_plaintext(&mut self, content: &[u8]) {
        let reader = std::io::BufReader::new(content);
        for line in reader.lines().map_while(Result::ok) {
            let line = line.trim().to_string();
            if !line.is_empty() {
                self.insert_entry(HistoryEntry {
                    command: line,
                    timestamp: 0,
                    frequency: 1,
                    exit_status: None,
                    cwd: None,
                });
            }
        }
    }

    fn insert_entry(&mut self, entry: HistoryEntry) {
        if let Some(&idx) = self.dedup_index.get(&entry.command) {
            // Update existing entry with newer timestamp and increment frequency
            let existing = &mut self.entries[idx];
            if entry.timestamp > existing.timestamp {
                existing.timestamp = entry.timestamp;
            }
            existing.frequency += 1;
            if entry.exit_status.is_some() {
                existing.exit_status = entry.exit_status;
            }
            if entry.cwd.is_some() {
                existing.cwd = entry.cwd;
            }
        } else {
            let idx = self.entries.len();
            self.dedup_index.insert(entry.command.clone(), idx);
            self.entries.push(entry);
        }
    }

    // --- Public API ---

    /// All history entries, oldest first.
    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    /// Total number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Push a new command to history.
    pub fn push(&mut self, command: String) {
        self.push_with_metadata(command, None, None);
    }

    /// Push a new command with exit status + cwd metadata attached —
    /// typically called from an OSC-133 `C` (CommandEnd) handler that
    /// captured the exit code and the `$PWD` of the command.
    pub fn push_with_metadata(
        &mut self,
        command: String,
        exit_status: Option<i32>,
        cwd: Option<String>,
    ) {
        if command.trim().is_empty() {
            return;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.insert_entry(HistoryEntry {
            command,
            timestamp: now,
            frequency: 1,
            exit_status,
            cwd,
        });
    }

    /// Frecency-scored prefix search. Returns entries sorted by score (highest first).
    pub fn frecency_search(&self, prefix: &str, limit: usize) -> Vec<&HistoryEntry> {
        if prefix.is_empty() {
            return Vec::new();
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let mut matches: Vec<(&HistoryEntry, f64)> = self
            .entries
            .iter()
            .filter(|e| e.command.starts_with(prefix) && e.command != prefix)
            .map(|e| {
                let age_secs = now.saturating_sub(e.timestamp);
                let recency = match age_secs {
                    0..=3600 => 4.0,
                    3601..=86400 => 2.0,
                    86401..=604800 => 1.0,
                    604801..=2592000 => 0.5,
                    _ => 0.25,
                };
                (e, e.frequency as f64 * recency)
            })
            .collect();

        matches.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        matches.into_iter().take(limit).map(|(e, _)| e).collect()
    }

    /// Fuzzy filter for history panel. Returns matching entries, newest first.
    pub fn fuzzy_filter(&self, query: &str) -> Vec<&HistoryEntry> {
        if query.is_empty() {
            return self.entries.iter().rev().collect();
        }
        let query_lower = query.to_lowercase();
        let mut matches: Vec<&HistoryEntry> = self
            .entries
            .iter()
            .filter(|e| e.command.to_lowercase().contains(&query_lower))
            .collect();
        matches.reverse(); // newest first
        matches
    }

    /// Fuzzy-score every entry against `query` and return ranked
    /// matches (highest score first). Empty query yields every entry
    /// with score 0, newest first.
    ///
    /// Scoring combines: base per-char match, consecutive-char bonus,
    /// word-boundary bonus, gap penalty, length bonus, and a small
    /// success/failure weight based on `exit_status`. Designed for
    /// Ctrl-R-style incremental search over ≤100k entries on every
    /// keystroke (typically under 5 ms p99).
    pub fn fuzzy_search(&self, query: &str, limit: usize) -> Vec<FuzzyMatch<'_>> {
        if query.is_empty() {
            return self
                .entries
                .iter()
                .rev()
                .take(limit)
                .map(|entry| FuzzyMatch {
                    entry,
                    score: 0.0,
                    positions: Vec::new(),
                })
                .collect();
        }
        let mut matches: Vec<FuzzyMatch<'_>> = self
            .entries
            .iter()
            .rev()
            .filter_map(|entry| fuzzy_score(query, entry))
            .collect();
        matches.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        matches.truncate(limit);
        matches
    }
}

/// Simple subsequence scorer with bonuses for consecutive matches
/// and word-boundary hits. Returns `None` when the query does not
/// subsequence-match the command.
fn fuzzy_score<'a>(query: &str, entry: &'a HistoryEntry) -> Option<FuzzyMatch<'a>> {
    let q_lower: Vec<char> = query.to_lowercase().chars().collect();
    if q_lower.is_empty() {
        return None;
    }
    let cmd = &entry.command;
    let mut cmd_chars = cmd.char_indices();
    let mut positions = Vec::with_capacity(q_lower.len());
    let mut prev_match_byte: Option<usize> = None;
    let mut score: f64 = 0.0;
    for &qc in &q_lower {
        let mut found = false;
        for (i, c) in cmd_chars.by_ref() {
            if c.to_lowercase().next() == Some(qc) {
                let consecutive = prev_match_byte.is_some_and(|prev| i == prev + c.len_utf8());
                let is_word_boundary = i == 0
                    || cmd[..i]
                        .chars()
                        .next_back()
                        .is_some_and(|prev| prev.is_whitespace() || !prev.is_alphanumeric());
                let mut delta: f64 = 4.0;
                if consecutive {
                    delta += 6.0;
                }
                if is_word_boundary {
                    delta += 8.0;
                }
                if let Some(prev) = prev_match_byte {
                    delta -= ((i - prev) as f64 - 1.0).clamp(0.0, 10.0);
                } else {
                    delta -= (i as f64).min(6.0);
                }
                score += delta;
                positions.push(i);
                prev_match_byte = Some(i);
                found = true;
                break;
            }
        }
        if !found {
            return None;
        }
    }
    score += ((100.0 - (cmd.len() as f64).min(100.0)) / 4.0).max(0.0);
    match entry.exit_status {
        Some(0) => score += 8.0,
        Some(_) => score -= 4.0,
        None => {}
    }
    Some(FuzzyMatch {
        entry,
        score,
        positions,
    })
}

/// Format a timestamp as relative time: "just now", "5m ago", "3h ago", "2d ago".
pub fn relative_time(timestamp: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let age = now.saturating_sub(timestamp);

    if age < 60 {
        "just now".to_string()
    } else if age < 3600 {
        format!("{}m ago", age / 60)
    } else if age < 86400 {
        format!("{}h ago", age / 3600)
    } else if age < 604800 {
        format!("{}d ago", age / 86400)
    } else if age < 2592000 {
        format!("{}w ago", age / 604800)
    } else {
        format!("{}mo ago", age / 2592000)
    }
}

/// App-level pointer to the shell [`CommandHistory`] backing the
/// currently-focused terminal pane.
///
/// Set by `TerminalPane` on focus-in so the command palette's History
/// source can show commands from the pane the user is actively working
/// in — not just whatever the shell flushed to disk.
#[derive(Clone)]
pub struct ActiveCommandHistory(pub Arc<RwLock<CommandHistory>>);

impl Global for ActiveCommandHistory {}

impl ActiveCommandHistory {
    pub fn try_global(cx: &App) -> Option<Self> {
        cx.try_global::<Self>().cloned()
    }

    pub fn set_global(history: Arc<RwLock<CommandHistory>>, cx: &mut App) {
        cx.set_global(Self(history));
    }
}

/// App-level snapshot of where the currently-focused terminal pane is
/// working — both the literal `cwd` and the `project_root` that the
/// shell-side classifier picked for it (the nearest `.git`, agent-rules
/// file, or package-manifest directory; falls back to `cwd` for shells
/// that aren't inside any project).
///
/// Set by `TerminalPane` on focus-in and on every cwd change while
/// focused. Authoritative because it bypasses `Project::visible_worktrees`,
/// whose contents lag behind the shell — `ensure_*_worktree` returns a
/// `Task` that resolves on the next tick, so a fresh `cd` into a project
/// is observable here several frames before the worktree shows up in the
/// store. Feature crates that need a scope for the active terminal
/// (e.g. the file-search source in the command palette) read
/// `project_root` directly.
#[derive(Clone)]
pub struct ActiveTerminalScope {
    pub cwd: std::path::PathBuf,
    pub project_root: std::path::PathBuf,
}

impl Global for ActiveTerminalScope {}

impl ActiveTerminalScope {
    pub fn try_global(cx: &App) -> Option<Self> {
        cx.try_global::<Self>().cloned()
    }

    pub fn set_global(
        cwd: std::path::PathBuf,
        project_root: std::path::PathBuf,
        cx: &mut App,
    ) {
        cx.set_global(Self { cwd, project_root });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_and_entries() {
        let mut history = CommandHistory::new();
        history.push("ls".into());
        history.push("cd ..".into());
        history.push("git status".into());

        assert_eq!(history.len(), 3);
        assert_eq!(history.entries()[0].command, "ls");
        assert_eq!(history.entries()[1].command, "cd ..");
        assert_eq!(history.entries()[2].command, "git status");
    }

    #[test]
    fn test_deduplication() {
        let mut history = CommandHistory::new();
        history.push("ls".into());
        history.push("cd".into());
        history.push("ls".into());

        assert_eq!(history.len(), 2); // "ls" deduplicated
        assert_eq!(history.entries[0].frequency, 2); // "ls" frequency bumped
    }

    #[test]
    fn test_frecency_search() {
        let mut history = CommandHistory::new();
        history.push("cargo build".into());
        history.push("cargo test".into());
        history.push("cargo run".into());
        history.push("cargo build".into()); // Repeated, higher frequency

        let results = history.frecency_search("cargo ", 10);
        // "cargo build" should rank higher due to frequency=2
        assert!(!results.is_empty());
        assert_eq!(results[0].command, "cargo build");
    }

    #[test]
    fn test_fuzzy_filter() {
        let mut history = CommandHistory::new();
        history.push("cargo build".into());
        history.push("git status".into());
        history.push("cargo test".into());

        let results = history.fuzzy_filter("cargo");
        assert_eq!(results.len(), 2);
        // Newest first
        assert_eq!(results[0].command, "cargo test");
        assert_eq!(results[1].command, "cargo build");
    }

    #[test]
    fn test_relative_time() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        assert_eq!(relative_time(now), "just now");
        assert_eq!(relative_time(now - 300), "5m ago");
        assert_eq!(relative_time(now - 7200), "2h ago");
        assert_eq!(relative_time(now - 172800), "2d ago");
    }

    #[test]
    fn test_parse_zsh_history() {
        let content = b": 1234567890:0;ls -la\n: 1234567891:0;cd ..\n";
        let mut history = CommandHistory::new();
        history.parse_zsh(content);
        assert_eq!(history.len(), 2);
        assert_eq!(history.entries[0].command, "ls -la");
        assert_eq!(history.entries[0].timestamp, 1234567890);
        assert_eq!(history.entries[1].command, "cd ..");
    }

    #[test]
    fn test_parse_bash_history() {
        let content = b"#1234567890\nls -la\ncd ..\n";
        let mut history = CommandHistory::new();
        history.parse_bash(content);
        assert_eq!(history.len(), 2);
        assert_eq!(history.entries[0].command, "ls -la");
        assert_eq!(history.entries[0].timestamp, 1234567890);
    }

    #[test]
    fn test_parse_fish_history() {
        let content = b"- cmd: ls -la\n  when: 1234567890\n- cmd: cd ..\n  when: 1234567891\n";
        let mut history = CommandHistory::new();
        history.parse_fish(content);
        assert_eq!(history.len(), 2);
        assert_eq!(history.entries[0].command, "ls -la");
        assert_eq!(history.entries[0].timestamp, 1234567890);
    }

    #[test]
    fn test_empty_commands_ignored() {
        let mut history = CommandHistory::new();
        history.push("".into());
        history.push("   ".into());
        assert_eq!(history.len(), 0);
    }

    #[test]
    fn from_command_builds_sensible_defaults() {
        let entry = HistoryEntry::from_command("ls -la");
        assert_eq!(entry.command, "ls -la");
        assert_eq!(entry.timestamp, 0);
        assert_eq!(entry.frequency, 1);
        assert_eq!(entry.exit_status, None);
        assert_eq!(entry.cwd, None);
    }

    fn seed() -> CommandHistory {
        let mut h = CommandHistory::new();
        h.insert_entry(HistoryEntry {
            exit_status: Some(0),
            ..HistoryEntry::from_command("git status")
        });
        h.insert_entry(HistoryEntry {
            exit_status: Some(0),
            ..HistoryEntry::from_command("git commit -m fix")
        });
        h.insert_entry(HistoryEntry {
            exit_status: Some(1),
            ..HistoryEntry::from_command("git push --force")
        });
        h.insert_entry(HistoryEntry {
            exit_status: Some(0),
            ..HistoryEntry::from_command("cargo test")
        });
        h
    }

    #[test]
    fn fuzzy_search_empty_query_returns_newest_first() {
        let h = seed();
        let results = h.fuzzy_search("", 10);
        let commands: Vec<&str> = results.iter().map(|m| m.entry.command.as_str()).collect();
        assert_eq!(
            commands,
            vec![
                "cargo test",
                "git push --force",
                "git commit -m fix",
                "git status"
            ]
        );
    }

    #[test]
    fn fuzzy_search_non_matching_returns_empty() {
        let h = seed();
        assert!(h.fuzzy_search("xyzqq", 10).is_empty());
    }

    #[test]
    fn fuzzy_search_success_ranks_above_failure() {
        let h = seed();
        let results = h.fuzzy_search("git", 10);
        let top = &results[0];
        assert_ne!(top.entry.command, "git push --force");
    }

    #[test]
    fn fuzzy_search_respects_limit() {
        let h = seed();
        let results = h.fuzzy_search("", 2);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn fuzzy_search_positions_are_ascending_and_in_range() {
        let h = seed();
        let results = h.fuzzy_search("gts", 10);
        for m in &results {
            for win in m.positions.windows(2) {
                assert!(win[0] < win[1]);
            }
            for &i in &m.positions {
                assert!(i < m.entry.command.len());
            }
        }
    }

    #[test]
    fn fuzzy_search_is_case_insensitive() {
        let mut h = CommandHistory::new();
        h.push("MakeFile".into());
        let results = h.fuzzy_search("make", 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry.command, "MakeFile");
    }
}

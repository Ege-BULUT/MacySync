//! The receiving side's setup: who sets things up after the files land. Each preset runs in the transfer's
//! folder (`{dir}`), where MacySync writes AGENTS.md and GOAL.md for it.

pub const ASK: &str = "Read AGENTS.md and GOAL.md in this folder and do what they say.";

/// (name, command). Every one is a plain shell command the person can change in the app.
pub const AGENTS: [(&str, &str); 5] = [
    ("Claude Code", "cd {dir} && claude --permission-mode acceptEdits \"Read AGENTS.md and GOAL.md in this folder and do what they say.\""),
    ("Codex", "cd {dir} && codex --full-auto \"Read AGENTS.md and GOAL.md in this folder and do what they say.\""),
    ("OpenRouter", "cd {dir} && ANTHROPIC_BASE_URL=https://openrouter.ai/api ANTHROPIC_AUTH_TOKEN=$OPENROUTER_API_KEY claude \"Read AGENTS.md and GOAL.md in this folder and do what they say.\""),
    ("Ollama", "cd {dir} && ANTHROPIC_BASE_URL=http://localhost:11434 ANTHROPIC_AUTH_TOKEN=ollama claude --model qwen3-coder \"Read AGENTS.md and GOAL.md in this folder and do what they say.\""),
    ("Custom", "cd {dir} && "),
];

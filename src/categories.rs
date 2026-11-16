//! Smart categorization of GitHub starred repositories.
//!
//! Each repo is scored against every category using three signal sources:
//!   1. GitHub topics (highest weight, explicit labels chosen by the owner)
//!   2. Primary language
//!   3. Whole-word keyword matches against the repo *name* (not the owner)
//!      and description (lowest weight)
//!
//! Keywords match on word boundaries after normalizing everything that is not
//! alphanumeric to a space, so `rust` no longer matches "trust", `cli` no
//! longer matches "client", and multi-word keywords such as `terminal ui`
//! match as phrases. A trailing `s` is accepted for simple plurals.
//!
//! The highest score wins; ties go to the category defined first; a score of
//! zero means "Other".

use crate::cache::StarredRepo;

/// Bump whenever the rules change so cached categories are recomputed.
pub const RULES_VERSION: u32 = 3;

/// All recognized categories, in display/priority order.
pub const CATEGORIES: &[&str] = &[
    "Rust",
    "Python",
    "AI / ML",
    "Security",
    "Systems / OS",
    "Games / Emulation",
    "Hardware / FPGA",
    "TUI / GUI",
    "DevTools",
    "Networking",
    "Databases",
    "Web / Frontend",
    "Other",
];

/// Scoring rules for a single category.
struct CategoryRule {
    name: &'static str,
    /// GitHub topic slugs (exact match, lowercased).
    topics: &'static [&'static str],
    /// Primary language names (case-insensitive).
    languages: &'static [&'static str],
    /// Words or phrases matched on word boundaries against name + description.
    keywords: &'static [&'static str],
    topic_weight: u32,
    language_weight: u32,
    keyword_weight: u32,
}

/// Lowercase and replace every non-alphanumeric char with a space, padded so
/// `" word "` substring tests are word-boundary tests.
fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push(' ');
    let mut last_space = true;
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    if !last_space {
        out.push(' ');
    }
    out
}

/// True when `phrase` (already lowercase, words separated by single spaces or
/// hyphens) occurs in normalized `text` on word boundaries, optionally plural.
fn contains_phrase(text: &str, phrase: &str) -> bool {
    let p = normalize(phrase);
    let p = p.trim();
    if p.is_empty() {
        return false;
    }
    text.contains(&format!(" {p} ")) || text.contains(&format!(" {p}s "))
}

impl CategoryRule {
    fn score(&self, topics: &[String], language: Option<&str>, text: &str) -> u32 {
        let mut score = 0u32;

        for topic in topics {
            if self.topics.iter().any(|&t| t.eq_ignore_ascii_case(topic)) {
                score += self.topic_weight;
            }
        }

        if let Some(lang) = language
            && self.languages.iter().any(|&l| l.eq_ignore_ascii_case(lang))
        {
            score += self.language_weight;
        }

        for kw in self.keywords {
            if contains_phrase(text, kw) {
                score += self.keyword_weight;
            }
        }

        score
    }
}

static RULES: &[CategoryRule] = &[
    CategoryRule {
        name: "Rust",
        topics: &[
            "rust",
            "rust-lang",
            "rustlang",
            "cargo",
            "crate",
            "async-rust",
            "embedded-rust",
            "rust-crate",
            "rust-library",
        ],
        languages: &["Rust"],
        keywords: &["rust", "cargo", "crate", "rustacean", "rustc"],
        topic_weight: 10,
        language_weight: 8,
        keyword_weight: 2,
    },
    // Mirrors the Rust category. In v0.1, Python as a language counted toward
    // AI / ML, which pulled every unlabelled Python repo in there; Python repos
    // now land here unless AI topics/keywords outweigh the language signal.
    CategoryRule {
        name: "Python",
        topics: &[
            "python",
            "python3",
            "python-3",
            "python-library",
            "python-package",
            "django",
            "flask",
            "fastapi",
            "pypi",
            "pip",
            "asyncio",
        ],
        languages: &["Python", "Cython"],
        keywords: &[
            "python", "django", "flask", "fastapi", "pypi", "pip", "pythonic",
        ],
        topic_weight: 10,
        language_weight: 8,
        keyword_weight: 2,
    },
    CategoryRule {
        name: "AI / ML",
        topics: &[
            "machine-learning",
            "deep-learning",
            "neural-network",
            "llm",
            "llms",
            "nlp",
            "computer-vision",
            "reinforcement-learning",
            "ai",
            "artificial-intelligence",
            "pytorch",
            "tensorflow",
            "transformers",
            "diffusion",
            "stable-diffusion",
            "gpt",
            "llama",
            "ggml",
            "gguf",
            "vllm",
            "rag",
            "agents",
            "ai-agents",
            "mcp",
            "generative-ai",
            "openai",
            "anthropic",
            "claude",
        ],
        languages: &["CUDA", "Jupyter Notebook"],
        keywords: &[
            "llm",
            "gpt",
            "neural network",
            "machine learning",
            "deep learning",
            "diffusion",
            "transformer",
            "embedding",
            "onnx",
            "vllm",
            "llama",
            "mistral",
            "gemini",
            "claude",
            "openai",
            "anthropic",
            "ai agent",
            "rag",
            "mcp server",
            "model context protocol",
            "inference engine",
            "fine tuning",
            // v0.1 keywords, now whole-word only
            "neural",
            "inference",
        ],
        topic_weight: 10,
        language_weight: 3,
        keyword_weight: 3,
    },
    CategoryRule {
        name: "Security",
        topics: &[
            "security",
            "cybersecurity",
            "infosec",
            "cryptography",
            "crypto",
            "tls",
            "ssl",
            "ctf",
            "pentest",
            "pentesting",
            "penetration-testing",
            "vulnerability",
            "exploit",
            "malware",
            "reverse-engineering",
            "fuzzing",
            "authentication",
            "authorization",
            "audit",
            "zero-trust",
            "hardening",
            "firewall",
            "red-team",
            "blue-team",
            "osint",
        ],
        languages: &["YARA"],
        keywords: &[
            "security",
            "cryptography",
            "exploit",
            "vulnerability",
            "pentest",
            "penetration testing",
            "ctf",
            "fuzzer",
            "fuzzing",
            "malware",
            "reverse engineering",
            "tls",
            "encryption",
            "zero trust",
            "hardening",
            "sql injection",
            "red team",
            "osint",
            "password",
            // v0.1 keywords, now whole-word only
            "crypto",
            "vuln",
            "ssl",
            "audit",
            "signing",
        ],
        topic_weight: 10,
        language_weight: 2,
        keyword_weight: 3,
    },
    CategoryRule {
        name: "Systems / OS",
        topics: &[
            "operating-system",
            "kernel",
            "os",
            "systems-programming",
            "bare-metal",
            "embedded",
            "rtos",
            "microkernel",
            "driver",
            "firmware",
            "bootloader",
            "hypervisor",
            "assembly",
            "low-level",
            "linux-kernel",
            "osdev",
        ],
        languages: &["C", "C++", "Assembly", "Zig"],
        keywords: &[
            "kernel",
            "syscall",
            "driver",
            "firmware",
            "bootloader",
            "hypervisor",
            "bare metal",
            "embedded",
            "rtos",
            "microkernel",
            "operating system",
            "allocator",
            "interrupt",
            "mmu",
            "uefi",
            "bios",
            "osdev",
            "scheduler",
        ],
        topic_weight: 10,
        language_weight: 2,
        keyword_weight: 3,
    },
    CategoryRule {
        name: "Games / Emulation",
        topics: &[
            "game",
            "games",
            "gamedev",
            "game-development",
            "emulator",
            "emulation",
            "nes",
            "snes",
            "n64",
            "gameboy",
            "retro",
            "retrogaming",
            "game-engine",
            "opengl",
            "vulkan",
            "bevy",
            "godot",
        ],
        languages: &["GDScript"],
        keywords: &[
            "game",
            "emulator",
            "emulation",
            "nes",
            "snes",
            "n64",
            "gameboy",
            "game boy",
            "retro",
            "sprite",
            "shader",
            "vulkan",
            "opengl",
            "bevy",
            "godot",
            "unity engine",
            "unity",
            "game engine",
            "roguelike",
        ],
        topic_weight: 10,
        language_weight: 6,
        keyword_weight: 3,
    },
    CategoryRule {
        name: "Hardware / FPGA",
        topics: &[
            "fpga",
            "verilog",
            "vhdl",
            "hdl",
            "hardware",
            "mcu",
            "microcontroller",
            "arduino",
            "raspberry-pi",
            "risc-v",
            "rtl",
            "asic",
            "pcb",
            "electronics",
            "esp32",
            "mister",
        ],
        languages: &["Verilog", "VHDL", "SystemVerilog"],
        keywords: &[
            "fpga",
            "verilog",
            "vhdl",
            "hdl",
            "rtl",
            "asic",
            "microcontroller",
            "arduino",
            "raspberry pi",
            "riscv",
            "risc v",
            "mister",
            "mcu",
            "esp32",
            "pcb",
            "software defined radio",
            "sdr",
        ],
        topic_weight: 10,
        language_weight: 8,
        keyword_weight: 4,
    },
    CategoryRule {
        name: "TUI / GUI",
        topics: &[
            "tui",
            "terminal",
            "cli-ui",
            "gui",
            "widget",
            "ratatui",
            "cursive",
            "egui",
            "tauri",
            "iced",
            "gtk",
            "qt",
            "terminal-ui",
            "desktop-app",
            "wasm",
        ],
        languages: &["QML"],
        keywords: &[
            "tui",
            "terminal ui",
            "terminal user interface",
            "ratatui",
            "cursive",
            "crossterm",
            "egui",
            "tauri",
            "iced",
            "widget",
            "dashboard",
            "gui",
            "repl",
            "interactive",
        ],
        topic_weight: 10,
        language_weight: 4,
        keyword_weight: 4,
    },
    CategoryRule {
        name: "DevTools",
        topics: &[
            "cli",
            "tool",
            "tools",
            "toolchain",
            "build-tool",
            "linter",
            "formatter",
            "debugger",
            "profiler",
            "benchmark",
            "ci",
            "cd",
            "devops",
            "automation",
            "scripting",
            "package-manager",
            "developer-tools",
            "git",
            "dotfiles",
        ],
        languages: &["Shell", "Makefile", "Dockerfile", "Nix", "PowerShell"],
        keywords: &[
            "tool",
            "cli",
            "command line",
            "linter",
            "formatter",
            "debugger",
            "profiler",
            "benchmark",
            "build system",
            "compiler",
            "automation",
            "workflow",
            "pipeline",
            "generator",
            "scaffold",
            "git",
            "dotfiles",
            "devops",
            // v0.1 keywords, now whole-word only
            "lint",
            "debug",
            "bench",
            "build",
            "compile",
            "script",
        ],
        topic_weight: 8,
        language_weight: 3,
        keyword_weight: 2,
    },
    CategoryRule {
        name: "Networking",
        topics: &[
            "networking",
            "network",
            "http",
            "http2",
            "http3",
            "grpc",
            "tcp",
            "udp",
            "proxy",
            "dns",
            "protocol",
            "socket",
            "p2p",
            "vpn",
            "tunnel",
            "wireguard",
            "ad-blocker",
        ],
        languages: &[],
        keywords: &[
            "network",
            "networking",
            "http",
            "grpc",
            "proxy",
            "dns",
            "protocol",
            "socket",
            "tcp",
            "udp",
            "tunnel",
            "vpn",
            "p2p",
            "bandwidth",
            "packet",
            "wireguard",
            "ad blocking",
            "advertisements",
            "latency",
        ],
        topic_weight: 10,
        language_weight: 2,
        keyword_weight: 3,
    },
    CategoryRule {
        name: "Databases",
        topics: &[
            "database",
            "databases",
            "db",
            "sql",
            "nosql",
            "storage",
            "kv-store",
            "key-value",
            "timeseries",
            "time-series",
            "sqlite",
            "postgres",
            "postgresql",
            "mysql",
            "redis",
            "search-engine",
            "cache",
            "search",
        ],
        languages: &["PLpgSQL", "SQL", "TSQL"],
        keywords: &[
            "database",
            "db",
            "sql",
            "nosql",
            "key value store",
            "sqlite",
            "postgres",
            "postgresql",
            "mysql",
            "redis",
            "time series",
            "search engine",
            "orm",
            // v0.1 keywords, now whole-word only
            "storage",
            "cache",
            "query",
            "transaction",
        ],
        topic_weight: 10,
        language_weight: 5,
        keyword_weight: 3,
    },
    CategoryRule {
        name: "Web / Frontend",
        topics: &[
            "web",
            "frontend",
            "html",
            "css",
            "javascript",
            "typescript",
            "react",
            "vue",
            "svelte",
            "webassembly",
            "wasm",
            "nodejs",
            "node",
            "deno",
            "nextjs",
            "tailwindcss",
        ],
        languages: &[
            "JavaScript",
            "TypeScript",
            "HTML",
            "CSS",
            "Vue",
            "Svelte",
            "Astro",
        ],
        keywords: &[
            "frontend",
            "html",
            "css",
            "react",
            "vue",
            "svelte",
            "nextjs",
            "next js",
            "nodejs",
            "node js",
            "deno",
            "browser",
            "webassembly",
            "wasm",
            "webgl",
            "web app",
            "website",
            "dom",
        ],
        topic_weight: 10,
        language_weight: 6,
        keyword_weight: 2,
    },
];

/// The text keywords are matched against: the repo name (owner excluded) and
/// its description, normalized for word-boundary matching.
fn keyword_text(repo: &StarredRepo) -> String {
    let name = repo
        .full_name
        .split_once('/')
        .map(|(_, n)| n)
        .unwrap_or(&repo.full_name);
    normalize(&format!(
        "{} {}",
        name,
        repo.description.as_deref().unwrap_or("")
    ))
}

/// Assign the best-matching category to a repo.
pub fn categorize(repo: &StarredRepo) -> String {
    let text = keyword_text(repo);
    let mut best_name = "Other";
    let mut best_score = 0u32;

    for rule in RULES {
        let score = rule.score(&repo.topics, repo.language.as_deref(), &text);
        if score > best_score {
            best_score = score;
            best_name = rule.name;
        }
    }

    best_name.to_string()
}

/// `(category_name, count)` pairs in [`CATEGORIES`] order, followed by any
/// unknown categories in name order. Categories with no repos are omitted.
pub fn category_counts(repos: &[StarredRepo]) -> Vec<(String, usize)> {
    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for r in repos {
        *counts.entry(r.category.as_str()).or_default() += 1;
    }

    let mut result: Vec<(String, usize)> = CATEGORIES
        .iter()
        .filter_map(|&c| counts.get(c).map(|&n| (c.to_string(), n)))
        .collect();

    for (cat, count) in &counts {
        if !CATEGORIES.contains(cat) {
            result.push((cat.to_string(), *count));
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(full_name: &str, desc: &str, lang: Option<&str>, topics: &[&str]) -> StarredRepo {
        StarredRepo {
            full_name: full_name.to_string(),
            description: Some(desc.to_string()),
            language: lang.map(str::to_string),
            topics: topics.iter().map(|s| s.to_string()).collect(),
            ..StarredRepo::test_default()
        }
    }

    #[test]
    fn normalize_pads_and_lowercases() {
        assert_eq!(normalize("Hello, World-2!"), " hello world 2 ");
    }

    #[test]
    fn phrase_matching_respects_word_boundaries() {
        let t = normalize("A trusted client for kubernetes business");
        assert!(!contains_phrase(&t, "rust"));
        assert!(!contains_phrase(&t, "cli"));
        assert!(!contains_phrase(&t, "nes"));
        assert!(contains_phrase(&t, "client"));
    }

    #[test]
    fn phrase_matching_handles_phrases_and_plurals() {
        let t = normalize("A terminal-UI for databases");
        assert!(contains_phrase(&t, "terminal ui"));
        assert!(contains_phrase(&t, "database"));
    }

    #[test]
    fn substring_false_positives_are_gone() {
        let cases = [
            (
                "acme/kube-tool",
                "Deploy to Kubernetes for your business",
                Some("Go"),
            ),
            (
                "acme/community",
                "A community-driven list of resources",
                None,
            ),
            (
                "acme/basic",
                "Basic information and formatting helpers",
                None,
            ),
        ];
        for (name, desc, lang) in cases {
            let r = repo(name, desc, lang, &[]);
            let cat = categorize(&r);
            assert_ne!(cat, "Games / Emulation", "{name} -> {cat}");
            assert_ne!(cat, "Hardware / FPGA", "{name} -> {cat}");
        }
    }

    #[test]
    fn owner_name_is_ignored() {
        // The owner "rustfans" must not pull this repo into Rust.
        let r = repo(
            "rustfans/pytorch-tricks",
            "Deep learning tricks",
            Some("Jupyter Notebook"),
            &[],
        );
        assert_eq!(categorize(&r), "AI / ML");
    }

    #[test]
    fn plain_python_repo_is_python_not_ai() {
        let r = repo("someone/thing", "A small utility", Some("Python"), &[]);
        assert_eq!(categorize(&r), "Python");
    }

    #[test]
    fn python_ai_repo_with_ai_topics_is_ai() {
        let r = repo(
            "o/trainer",
            "Train models",
            Some("Python"),
            &["machine-learning", "pytorch"],
        );
        assert_eq!(categorize(&r), "AI / ML");
    }

    #[test]
    fn python_web_framework_topic_is_python() {
        let r = repo("o/site", "My site", None, &["django"]);
        assert_eq!(categorize(&r), "Python");
    }

    #[test]
    fn topics_dominate() {
        let r = repo("o/x", "", Some("Python"), &["fpga", "verilog"]);
        assert_eq!(categorize(&r), "Hardware / FPGA");
    }

    #[test]
    fn rust_language_wins_for_plain_rust_crate() {
        let r = repo("o/thing", "A fast thing", Some("Rust"), &[]);
        assert_eq!(categorize(&r), "Rust");
    }

    #[test]
    fn no_signal_is_other() {
        let r = repo("o/notes", "My notes", None, &[]);
        assert_eq!(categorize(&r), "Other");
    }

    #[test]
    fn counts_follow_category_order() {
        let mut a = repo("o/a", "", None, &[]);
        a.category = "Rust".into();
        let mut b = repo("o/b", "", None, &[]);
        b.category = "Other".into();
        let mut c = repo("o/c", "", None, &[]);
        c.category = "Rust".into();
        let counts = category_counts(&[b, a, c]);
        assert_eq!(counts, vec![("Rust".into(), 2), ("Other".into(), 1)]);
    }
}

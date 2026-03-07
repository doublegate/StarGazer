// categories.rs — Smart categorization of GitHub starred repositories.
//
// Strategy: score each repo across named categories using three signal sources:
//   1. GitHub topics (highest weight — explicit labels by the owner)
//   2. Primary language
//   3. Keyword match against full_name + description (lowest weight)
//
// The highest-scoring category wins. Ties go to the first defined category.

use crate::github::StarredRepo;

/// All recognized categories, in display/priority order.
pub const CATEGORIES: &[&str] = &[
    "Rust",
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

/// Rules for a single category: topic keywords, languages, and name/desc keywords.
/// Each match adds to a score; highest score wins.
struct CategoryRule {
    name: &'static str,
    /// GitHub topic slugs (exact match, lowercased)
    topics: &'static [&'static str],
    /// Primary language names (case-insensitive)
    languages: &'static [&'static str],
    /// Substrings matched against lowercased repo name + description
    keywords: &'static [&'static str],
    /// Extra weight for topic matches (vs keyword matches)
    topic_weight: u32,
    keyword_weight: u32,
    language_weight: u32,
}

impl CategoryRule {
    fn score(&self, repo: &StarredRepo) -> u32 {
        let mut score = 0u32;
        let name_lower = repo.full_name.to_lowercase();
        let desc_lower = repo
            .description
            .as_deref()
            .unwrap_or("")
            .to_lowercase();
        let text = format!("{name_lower} {desc_lower}");

        // Topic matches (highest signal)
        for topic in &repo.topics {
            let t = topic.to_lowercase();
            if self.topics.iter().any(|&s| s == t) {
                score += self.topic_weight;
            }
        }

        // Language match
        if let Some(ref lang) = repo.language {
            let lang_lower = lang.to_lowercase();
            if self
                .languages
                .iter()
                .any(|&l| l.to_lowercase() == lang_lower)
            {
                score += self.language_weight;
            }
        }

        // Keyword match in name/description
        for kw in self.keywords {
            if text.contains(kw) {
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
            "rust", "rust-lang", "rustlang", "cargo", "crate",
            "async-rust", "embedded-rust", "rust-crate",
        ],
        languages: &["Rust"],
        keywords: &["rust", "cargo", "crate", "rustacean"],
        topic_weight: 10,
        language_weight: 8,
        keyword_weight: 2,
    },
    CategoryRule {
        name: "AI / ML",
        topics: &[
            "machine-learning", "deep-learning", "neural-network",
            "llm", "nlp", "computer-vision", "reinforcement-learning",
            "ai", "artificial-intelligence", "pytorch", "tensorflow",
            "transformers", "diffusion", "stable-diffusion", "gpt",
            "llama", "inference", "ggml", "gguf", "vllm",
        ],
        languages: &["Python", "CUDA", "Jupyter Notebook"],
        keywords: &[
            "llm", "gpt", "neural", "inference", "model", "diffusion",
            "transformer", "embedding", "onnx", "vllm", "llama",
            "mistral", "gemini", "claude", "openai", "anthropic",
        ],
        topic_weight: 10,
        language_weight: 3,
        keyword_weight: 3,
    },
    CategoryRule {
        name: "Security",
        topics: &[
            "security", "cryptography", "crypto", "tls", "ssl",
            "ctf", "pentest", "vulnerability", "exploit",
            "malware", "reverse-engineering", "fuzzing",
            "authentication", "authorization", "audit",
            "zero-trust", "hardening", "firewall",
        ],
        languages: &[],
        keywords: &[
            "security", "crypto", "exploit", "vuln", "pentest",
            "ctf", "fuzzing", "malware", "reverse", "audit",
            "tls", "ssl", "encryption", "signing", "verify",
            "zero-trust", "hardening", "injection",
        ],
        topic_weight: 10,
        language_weight: 2,
        keyword_weight: 3,
    },
    CategoryRule {
        name: "Systems / OS",
        topics: &[
            "operating-system", "kernel", "os", "systems-programming",
            "bare-metal", "embedded", "rtos", "microkernel",
            "driver", "firmware", "bootloader", "hypervisor",
            "assembly", "low-level",
        ],
        languages: &["C", "C++", "Assembly", "Zig"],
        keywords: &[
            "kernel", "syscall", "driver", "firmware", "bootloader",
            "hypervisor", "bare-metal", "embedded", "rtos",
            "microkernel", "scheduler", "allocator", "memory",
            "interrupt", "mmu", "uefi", "bios",
        ],
        topic_weight: 10,
        language_weight: 4,
        keyword_weight: 3,
    },
    CategoryRule {
        name: "Games / Emulation",
        topics: &[
            "game", "gamedev", "game-development", "emulator",
            "emulation", "nes", "snes", "n64", "gameboy",
            "retro", "game-engine", "opengl", "vulkan", "bevy",
        ],
        languages: &[],
        keywords: &[
            "game", "emul", "nes", "snes", "n64", "gameboy",
            "retro", "sprite", "shader", "vulkan", "opengl",
            "bevy", "godot", "unity", "level", "render",
        ],
        topic_weight: 10,
        language_weight: 2,
        keyword_weight: 3,
    },
    CategoryRule {
        name: "Hardware / FPGA",
        topics: &[
            "fpga", "verilog", "vhdl", "hdl", "hardware",
            "mcu", "microcontroller", "arduino", "raspberry-pi",
            "arm", "risc-v", "rtl", "asic",
        ],
        languages: &["Verilog", "VHDL", "SystemVerilog"],
        keywords: &[
            "fpga", "verilog", "vhdl", "hdl", "rtl", "asic",
            "microcontroller", "arduino", "raspberry", "riscv",
            "risc-v", "mister", "mcu",
        ],
        topic_weight: 10,
        language_weight: 8,
        keyword_weight: 4,
    },
    CategoryRule {
        name: "TUI / GUI",
        topics: &[
            "tui", "terminal", "cli-ui", "gui", "widget",
            "ratatui", "cursive", "egui", "tauri", "iced",
            "gtk", "qt", "wasm",
        ],
        languages: &[],
        keywords: &[
            "tui", "terminal ui", "ratatui", "cursive", "crossterm",
            "egui", "tauri", "iced", "widget", "dashboard",
            "repl", "interactive",
        ],
        topic_weight: 10,
        language_weight: 2,
        keyword_weight: 4,
    },
    CategoryRule {
        name: "DevTools",
        topics: &[
            "cli", "tool", "toolchain", "build-tool", "linter",
            "formatter", "debugger", "profiler", "benchmark",
            "ci", "cd", "devops", "automation", "scripting",
            "package-manager",
        ],
        languages: &["Shell", "Makefile", "Dockerfile"],
        keywords: &[
            "tool", "cli", "lint", "format", "debug", "profile",
            "bench", "build", "compile", "script", "automati",
            "workflow", "pipeline", "generator", "scaffold",
        ],
        topic_weight: 8,
        language_weight: 3,
        keyword_weight: 2,
    },
    CategoryRule {
        name: "Networking",
        topics: &[
            "networking", "network", "http", "http2", "http3",
            "grpc", "tcp", "udp", "proxy", "dns", "protocol",
            "socket", "p2p", "vpn", "tunnel",
        ],
        languages: &[],
        keywords: &[
            "network", "http", "grpc", "proxy", "dns", "protocol",
            "socket", "tcp", "udp", "tunnel", "vpn", "p2p",
            "bandwidth", "latency", "packet",
        ],
        topic_weight: 10,
        language_weight: 2,
        keyword_weight: 3,
    },
    CategoryRule {
        name: "Databases",
        topics: &[
            "database", "db", "sql", "nosql", "storage",
            "cache", "kv-store", "timeseries", "search",
            "sqlite", "postgres", "mysql", "redis",
        ],
        languages: &["PLpgSQL", "SQL"],
        keywords: &[
            "database", " db", "sql", "storage", "cache",
            "index", "query", "transaction", "sqlite", "postgres",
            "redis", "timeseries", "search engine",
        ],
        topic_weight: 10,
        language_weight: 5,
        keyword_weight: 3,
    },
    CategoryRule {
        name: "Web / Frontend",
        topics: &[
            "web", "frontend", "html", "css", "javascript",
            "typescript", "react", "vue", "svelte",
            "webassembly", "wasm", "node", "deno",
        ],
        languages: &["JavaScript", "TypeScript", "HTML", "CSS"],
        keywords: &[
            "frontend", "html", "css", "react", "vue", "svelte",
            "nextjs", "nodejs", "deno", "browser", "dom",
            "wasm", "webgl",
        ],
        topic_weight: 10,
        language_weight: 6,
        keyword_weight: 2,
    },
];

/// Assign the best-matching category to a repo.
pub fn categorize(repo: &StarredRepo) -> String {
    let mut best_name = "Other";
    let mut best_score = 0u32;

    for rule in RULES {
        if rule.name == "Other" {
            continue;
        }
        let score = rule.score(repo);
        if score > best_score {
            best_score = score;
            best_name = rule.name;
        }
    }

    best_name.to_string()
}

/// Return a sorted list of (category_name, count) pairs for a repo slice.
pub fn category_counts(repos: &[StarredRepo]) -> Vec<(String, usize)> {
    let mut counts: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();

    for r in repos {
        *counts.entry(r.category.as_str()).or_default() += 1;
    }

    // Return in CATEGORIES order
    let mut result: Vec<(String, usize)> = CATEGORIES
        .iter()
        .filter_map(|&c| counts.get(c).map(|&n| (c.to_string(), n)))
        .collect();

    // Append any unknown categories not in the static list
    for (cat, count) in &counts {
        if !CATEGORIES.contains(cat) {
            result.push((cat.to_string(), *count));
        }
    }

    result
}

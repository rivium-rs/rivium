// Rejects a project name that would generate a workspace that cargo or rustc cannot build.
// cargo-generate has already turned the name into kebab case.
let name = variable::get("project-name");

// Packages of these names clash with a dependency of the same name, or with Rivium's crates.
let dependencies = ["axum", "serde", "tracing"];
// The library's crate name would be a keyword or a built-in crate, the binary's a directory of
// target/, or the crate's directory a device name on Windows.
let reserved = [
    "std", "core", "alloc", "test", "proc-macro",
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
    "extern", "false", "fn", "for", "gen", "if", "impl", "in", "let", "loop", "match", "mod",
    "move", "mut", "pub", "ref", "return", "self", "static", "struct", "super", "trait", "true",
    "try", "type", "unsafe", "use", "where", "while", "abstract", "become", "box", "do",
    "final", "macro", "override", "priv", "typeof", "unsized", "virtual", "yield",
    "build", "deps", "examples", "incremental",
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

let first = if name.is_empty() { ' ' } else { name[0] };
let problem = if first < 'a' || first > 'z' {
    "it must start with a lowercase letter"
} else if name == "rivium" || name.starts_with("rivium-") {
    "names starting with rivium belong to Rivium's crates"
} else if dependencies.contains(name) {
    "the project depends on a crate of this name"
} else if reserved.contains(name) {
    "the name is reserved by Rust, cargo or Windows"
} else {
    ""
};
if problem != "" {
    // cargo-generate has created the project's directory by now, unless it generates into the
    // current one (--init).
    let init = variable::is_set("is_init") && variable::get("is_init");
    let hint = if init { "" } else { `; remove the empty directory '${name}' it created` };
    abort(`invalid project name '${name}': ${problem}${hint}`);
}

file::delete("hooks");

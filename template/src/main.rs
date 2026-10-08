//! {{project-name}}: generated from the Rivium placeholder template.

fn main() {
    println!("{{project-name}} {}", env!("CARGO_PKG_VERSION"));
}

#[cfg(test)]
mod tests {
    #[test]
    fn links_against_rivium() {
        // The placeholder only checks that the rivium crates resolve and link.
        let _ = core::any::type_name::<fn()>();
    }
}

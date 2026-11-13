//! Turns each `KEY="value"` line of `brand.env` into the compile-time
//! variable `BRAND_<KEY>`, read by `src/branding.rs` with `env!`.

use std::{env, fs, path::Path, process::exit};

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let path = Path::new(&manifest_dir).join("brand.env");
    println!("cargo:rerun-if-changed=brand.env");
    println!("cargo:rerun-if-changed=build.rs");

    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) => fail(&format!("cannot read {}: {error}", path.display())),
    };

    let mut seen: Vec<&str> = Vec::new();
    for (index, line) in contents.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let location = format!("{}:{}", path.display(), index + 1);
        let Some((key, value)) = line.split_once('=') else {
            fail(&format!("{location}: expected KEY=\"value\""));
        };
        let key = key.trim();
        let valid_key = !key.is_empty()
            && key.starts_with(|c: char| c.is_ascii_uppercase())
            && key
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        if !valid_key {
            fail(&format!(
                "{location}: `{key}` is not an UPPER_SNAKE_CASE key"
            ));
        }
        if seen.contains(&key) {
            fail(&format!("{location}: `{key}` is defined twice"));
        }
        let value = value.trim();
        let Some(value) = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
        else {
            fail(&format!(
                "{location}: the value of `{key}` must be in double quotes"
            ));
        };
        if value.contains('"') || value.contains('\\') {
            fail(&format!(
                "{location}: the value of `{key}` must not contain `\"` or `\\`"
            ));
        }
        seen.push(key);
        println!("cargo:rustc-env=BRAND_{key}={value}");
    }
}

fn fail(message: &str) -> ! {
    eprintln!("branding: {message}");
    exit(1);
}

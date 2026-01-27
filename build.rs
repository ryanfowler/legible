use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("generated_tests.rs");

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let test_pages = Path::new(&manifest_dir).join("tests/readability-js/test/test-pages");

    let mut tests = String::new();

    if test_pages.exists() {
        let mut entries: Vec<_> = fs::read_dir(&test_pages)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .collect();
        entries.sort_by_key(|e| e.file_name());

        for entry in entries {
            let name = entry.file_name().to_string_lossy().to_string();
            // Convert to valid Rust identifier
            let fn_name = sanitize_test_name(&name);

            tests.push_str(&format!(
                r#"
#[test]
fn {fn_name}() {{
    let test_dir = get_test_pages_dir().join("{name}");
    if let Err(e) = run_test_case(&test_dir) {{
        panic!("{name} failed: {{}}", e);
    }}
}}
"#
            ));
        }
    }

    fs::write(&dest_path, tests).unwrap();
    println!("cargo:rerun-if-changed=tests/readability-js/test/test-pages");
}

fn sanitize_test_name(name: &str) -> String {
    let mut fn_name = name.replace("-", "_").replace(".", "_");
    if fn_name.chars().next().unwrap().is_numeric() {
        fn_name = format!("test_{}", fn_name);
    }
    fn_name
}

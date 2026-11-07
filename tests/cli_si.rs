//! Seam 3: `cli/si` reads what lager writes.
//!
//! Needs Python 3 with PyYAML (`cli/si` imports it); each test is skipped
//! with a message when they aren't available.

mod common;

use std::path::Path;
use std::process::Command;

use common::{open, temp_db};

/// Why `cli/si` can't run here, if it can't.
fn missing_python() -> Option<String> {
    match Command::new("python3").args(["-c", "import yaml"]).output() {
        Err(e) => Some(format!("python3 is not available ({e})")),
        Ok(out) if !out.status.success() => Some(format!(
            "python3 can't import PyYAML: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Ok(_) => None,
    }
}

/// Runs `cli/si --database <database> <args>` and parses its JSON output.
fn run_si(database: &Path, args: &[&str]) -> serde_json::Value {
    let out = Command::new("python3")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("cli/si"))
        .arg("--database")
        .arg(database)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "cli/si {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "cli/si {args:?} printed invalid JSON ({e}):\n{}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

#[tokio::test]
async fn lists_a_host_lager_created() {
    if let Some(reason) = missing_python() {
        eprintln!("SKIPPED lists_a_host_lager_created: {reason}");
        return;
    }
    let db = temp_db();
    open(&db).await.create_host("web1").await.unwrap();

    let listing = run_si(&db.path, &["--list"]);

    let hostvars = listing["_meta"]["hostvars"]
        .as_object()
        .unwrap_or_else(|| panic!("no _meta.hostvars in cli/si --list output:\n{listing:#}"));
    assert_eq!(hostvars.keys().collect::<Vec<_>>(), ["web1"], "{listing:#}");
}

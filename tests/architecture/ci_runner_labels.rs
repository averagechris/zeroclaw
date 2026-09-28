//! Architecture gate for fork-compatible Quality Gate runner selection.
//!
//! The fork has no repository runners, so every Linux job in the required CI
//! and advisory Windows selector must use a GitHub-hosted image. Rust cache
//! callers must select the corresponding GitHub-hosted cache implementation.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use regex::Regex;

const HOSTED_LINUX_LABELS: [&str; 2] = ["ubuntu-24.04", "ubuntu-latest"];

fn workflow(filename: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(".github/workflows");
    fs::read_to_string(root.join(filename))
        .unwrap_or_else(|error| panic!("failed to read {filename}: {error}"))
}

fn job_blocks(workflow: &str) -> BTreeMap<String, String> {
    let (_, jobs) = workflow
        .split_once("\njobs:\n")
        .expect("workflow must declare a top-level jobs mapping");
    let header = Regex::new(r"(?m)^  ([a-z0-9-]+):$").expect("valid job-header pattern");
    let starts: Vec<(usize, String)> = header
        .captures_iter(jobs)
        .map(|capture| {
            let whole = capture.get(0).expect("match 0 always exists");
            (whole.start(), capture[1].to_string())
        })
        .collect();
    assert!(!starts.is_empty(), "workflow must define at least one job");

    let mut blocks = BTreeMap::new();
    for (index, (offset, name)) in starts.iter().enumerate() {
        let end = starts.get(index + 1).map_or(jobs.len(), |(next, _)| *next);
        assert!(
            blocks
                .insert(name.clone(), jobs[*offset..end].to_string())
                .is_none(),
            "duplicate job id {name}"
        );
    }
    blocks
}

fn needs(block: &str) -> Vec<String> {
    let Some(line) = block.lines().find(|line| line.starts_with("    needs: [")) else {
        return Vec::new();
    };
    line.trim_start()
        .trim_start_matches("needs: [")
        .trim_end_matches(']')
        .split(',')
        .map(|entry| entry.trim().to_string())
        .filter(|entry| !entry.is_empty())
        .collect()
}

#[test]
fn linux_jobs_use_only_github_hosted_runners() {
    for filename in ["ci.yml", "windows-tests.yml"] {
        for (name, block) in job_blocks(&workflow(filename)) {
            for line in block
                .lines()
                .filter(|line| line.trim_start().starts_with("runs-on:"))
            {
                let label = line.trim_start().trim_start_matches("runs-on: ");
                if label == "${{ matrix.os }}"
                    || label.starts_with("windows-")
                    || label.starts_with("macos-")
                {
                    continue;
                }
                assert!(
                    HOSTED_LINUX_LABELS.contains(&label),
                    "{filename}/{name} uses unavailable runner {label}"
                );
            }
        }
    }
}

#[test]
fn build_linux_matrix_uses_a_hosted_runner() {
    let ci = workflow("ci.yml");
    let build = job_blocks(&ci)
        .remove("build")
        .expect("ci.yml must define build");
    assert!(
        HOSTED_LINUX_LABELS
            .iter()
            .any(|label| build.contains(&format!("- os: {label}\n"))),
        "the Linux build matrix entry must use a GitHub-hosted runner"
    );
}

#[test]
fn rust_cache_callers_select_the_github_provider() {
    let ci = workflow("ci.yml");
    let inputs: Vec<&str> = ci
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("use-blacksmith: "))
        .collect();
    assert!(
        !inputs.is_empty(),
        "CI must exercise the rust-cache composite"
    );
    assert!(
        inputs.iter().all(|input| *input == "'false'"),
        "GitHub-hosted jobs must not select the Blacksmith cache: {inputs:?}"
    );
}

#[test]
fn compile_jobs_do_not_wait_for_formatting() {
    let ci = workflow("ci.yml");
    let blocks = job_blocks(&ci);
    for name in [
        "lint",
        "build",
        "check",
        "check-plugin-backends",
        "msrv",
        "check-32bit",
        "bench",
        "test",
        "test-channel-features",
        "memory-postgres-test",
        "parallel-runtime-test",
        "installer-drift",
    ] {
        let block = blocks
            .get(name)
            .unwrap_or_else(|| panic!("missing CI job {name}"));
        assert!(
            !needs(block).iter().any(|dependency| dependency == "fmt"),
            "{name} must not wait for fmt"
        );
    }
}

#[test]
fn required_gate_still_waits_for_formatting() {
    let ci = workflow("ci.yml");
    let blocks = job_blocks(&ci);
    let gate = blocks.get("gate").expect("ci.yml must define gate");
    assert!(
        needs(gate).iter().any(|dependency| dependency == "fmt"),
        "CI Required Gate must keep formatting as a required result"
    );
}

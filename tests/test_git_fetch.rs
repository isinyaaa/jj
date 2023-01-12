// Copyright 2023 The Jujutsu Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::path::Path;

use crate::common::TestEnvironment;

pub mod common;

fn create_commit(test_env: &TestEnvironment, repo_path: &Path, name: &str, parents: &[&str]) {
    if parents.is_empty() {
        test_env.jj_cmd_success(repo_path, &["new", "root", "-m", name]);
    } else {
        let descr = format!("descr_for_{name}");
        let mut args = vec!["new", "-m", &descr];
        args.extend(parents);
        test_env.jj_cmd_success(repo_path, &args);
    }
    std::fs::write(repo_path.join(name), format!("{name}\n")).unwrap();
    test_env.jj_cmd_success(repo_path, &["branch", "create", name]);
}

fn get_log_output(test_env: &TestEnvironment, workspace_root: &Path) -> String {
    test_env.jj_cmd_success(
        workspace_root,
        &[
            "log",
            "-T",
            r#"commit_id.short() " " description.first_line() " " branches"#,
            "-r",
            "all()",
        ],
    )
}

fn create_colocated_repo_and_branches_from_trunk1(
    test_env: &TestEnvironment,
    repo_path: &Path,
) -> String {
    // Create a colocated repo in `source` to populate it more easily
    test_env.jj_cmd_success(repo_path, &["init", "--git-repo", "."]);
    create_commit(test_env, repo_path, "trunk1", &[]);
    create_commit(test_env, repo_path, "a1", &["trunk1"]);
    create_commit(test_env, repo_path, "a2", &["trunk1"]);
    create_commit(test_env, repo_path, "b", &["trunk1"]);
    format!(
        "   ===== Source git repo contents =====\n{}",
        get_log_output(test_env, repo_path)
    )
}

fn create_trunk2_and_rebase_branches(test_env: &TestEnvironment, repo_path: &Path) -> String {
    create_commit(test_env, repo_path, "trunk2", &["trunk1"]);
    for br in ["a1", "a2", "b"] {
        test_env.jj_cmd_success(repo_path, &["rebase", "-b", br, "-d", "trunk2"]);
    }
    format!(
        "   ===== Source git repo contents =====\n{}",
        get_log_output(test_env, repo_path)
    )
}

#[test]
fn test_git_fetch_all() {
    let test_env = TestEnvironment::default();
    let source_git_repo_path = test_env.env_root().join("source");
    let _git_repo = git2::Repository::init(source_git_repo_path.clone()).unwrap();

    // Clone an empty repo. The target repo is a normal `jj` repo, *not* colocated
    let stdout =
        test_env.jj_cmd_success(test_env.env_root(), &["git", "clone", "source", "target"]);
    insta::assert_snapshot!(stdout, @r###"
    Fetching into new repo in "$TEST_ENV/target"
    Nothing changed.
    "###);
    let target_jj_repo_path = test_env.env_root().join("target");

    let source_log =
        create_colocated_repo_and_branches_from_trunk1(&test_env, &source_git_repo_path);
    insta::assert_snapshot!(source_log, @r###"
       ===== Source git repo contents =====
    @ 43fe77b5815e descr_for_b b
    | o cb1fe5d97488 descr_for_a2 a2
    |/  
    | o 5f35ea16f836 descr_for_a1 a1
    |/  
    o 9929b494c411 trunk1 master trunk1
    o 000000000000 (no description set) 
    "###);

    // Nothing in our repo before the fetch
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r###"
    @ 230dd059e1b0 (no description set) 
    o 000000000000 (no description set) 
    "###);
    insta::assert_snapshot!(test_env.jj_cmd_success(&target_jj_repo_path, &["git", "fetch"]), @"");
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r###"
    o 43fe77b5815e descr_for_b b
    | o cb1fe5d97488 descr_for_a2 a2
    |/  
    | o 5f35ea16f836 descr_for_a1 a1
    |/  
    o 9929b494c411 trunk1 master trunk1
    | @ 230dd059e1b0 (no description set) 
    |/  
    o 000000000000 (no description set) 
    "###);

    // Change the target repo
    let source_log = create_trunk2_and_rebase_branches(&test_env, &source_git_repo_path);
    insta::assert_snapshot!(source_log, @r###"
       ===== Source git repo contents =====
    o 90ded7086076 descr_for_b b
    | o d81785eb05b3 descr_for_a2 a2
    |/  
    | o 4b6d31d2dbab descr_for_a1 a1
    |/  
    @ baf8ee894f05 descr_for_trunk2 trunk2
    o 9929b494c411 trunk1 master trunk1
    o 000000000000 (no description set) 
    "###);

    // Our repo before and after fetch
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r###"
    o 43fe77b5815e descr_for_b b
    | o cb1fe5d97488 descr_for_a2 a2
    |/  
    | o 5f35ea16f836 descr_for_a1 a1
    |/  
    o 9929b494c411 trunk1 master trunk1
    | @ 230dd059e1b0 (no description set) 
    |/  
    o 000000000000 (no description set) 
    "###);
    insta::assert_snapshot!(test_env.jj_cmd_success(&target_jj_repo_path, &["git", "fetch"]), @"");
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r###"
    o 90ded7086076 descr_for_b b
    | o d81785eb05b3 descr_for_a2 a2
    |/  
    | o 4b6d31d2dbab descr_for_a1 a1
    |/  
    o baf8ee894f05 descr_for_trunk2 trunk2
    o 9929b494c411 trunk1 master trunk1
    | @ 230dd059e1b0 (no description set) 
    |/  
    o 000000000000 (no description set) 
    "###);
}

#[test]
fn test_git_fetch_some() {
    let test_env = TestEnvironment::default();
    let source_git_repo_path = test_env.env_root().join("source");
    let _git_repo = git2::Repository::init(source_git_repo_path.clone()).unwrap();

    // Clone an empty repo. The target repo is a normal `jj` repo, *not* colocated
    let stdout =
        test_env.jj_cmd_success(test_env.env_root(), &["git", "clone", "source", "target"]);
    insta::assert_snapshot!(stdout, @r###"
    Fetching into new repo in "$TEST_ENV/target"
    Nothing changed.
    "###);
    let target_jj_repo_path = test_env.env_root().join("target");

    let source_log =
        create_colocated_repo_and_branches_from_trunk1(&test_env, &source_git_repo_path);
    insta::assert_snapshot!(source_log, @r###"
       ===== Source git repo contents =====
    @ 43fe77b5815e descr_for_b b
    | o cb1fe5d97488 descr_for_a2 a2
    |/  
    | o 5f35ea16f836 descr_for_a1 a1
    |/  
    o 9929b494c411 trunk1 master trunk1
    o 000000000000 (no description set) 
    "###);

    // Nothing in our repo before the fetch
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r###"
    @ 230dd059e1b0 (no description set) 
    o 000000000000 (no description set) 
    "###);
    let stdout = test_env.jj_cmd_success(&target_jj_repo_path, &["git", "fetch", "--glob", "b"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r###"
    o 43fe77b5815e descr_for_b b
    o 9929b494c411 trunk1 
    | @ 230dd059e1b0 (no description set) 
    |/  
    o 000000000000 (no description set) 
    "###);
    let stdout = test_env.jj_cmd_success(&target_jj_repo_path, &["git", "fetch", "--glob", "a*"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r###"
    o cb1fe5d97488 descr_for_a2 a2
    | o 5f35ea16f836 descr_for_a1 a1
    |/  
    | o 43fe77b5815e descr_for_b b
    |/  
    o 9929b494c411 trunk1 
    | @ 230dd059e1b0 (no description set) 
    |/  
    o 000000000000 (no description set) 
    "###);
    let stdout = test_env.jj_cmd_success(&target_jj_repo_path, &["git", "fetch", "--glob", "a1"]);
    insta::assert_snapshot!(stdout, @r###"
    Nothing changed.
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r###"
    o cb1fe5d97488 descr_for_a2 a2
    | o 5f35ea16f836 descr_for_a1 a1
    |/  
    | o 43fe77b5815e descr_for_b b
    |/  
    o 9929b494c411 trunk1 
    | @ 230dd059e1b0 (no description set) 
    |/  
    o 000000000000 (no description set) 
    "###);

    // Change the target repo
    let source_log = create_trunk2_and_rebase_branches(&test_env, &source_git_repo_path);
    insta::assert_snapshot!(source_log, @r###"
       ===== Source git repo contents =====
    o f561acfed1d2 descr_for_b b
    | o bae45043dbd2 descr_for_a2 a2
    |/  
    | o 51fa81c130b6 descr_for_a1 a1
    |/  
    @ c0bc3b78f807 descr_for_trunk2 trunk2
    o 9929b494c411 trunk1 master trunk1
    o 000000000000 (no description set) 
    "###);

    // Our repo before and after fetch
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r###"
    o cb1fe5d97488 descr_for_a2 a2
    | o 5f35ea16f836 descr_for_a1 a1
    |/  
    | o 43fe77b5815e descr_for_b b
    |/  
    o 9929b494c411 trunk1 
    | @ 230dd059e1b0 (no description set) 
    |/  
    o 000000000000 (no description set) 
    "###);
    let stdout = test_env.jj_cmd_success(
        &target_jj_repo_path,
        &["git", "fetch", "--glob", "b", "--glob", "a1"],
    );
    insta::assert_snapshot!(stdout, @"");
    // TODO: Is it a bug that the old commits are not abandoned?
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r###"
    o f561acfed1d2 descr_for_b b
    | o 51fa81c130b6 descr_for_a1 a1
    |/  
    o c0bc3b78f807 descr_for_trunk2 
    | o cb1fe5d97488 descr_for_a2 a2
    |/  
    | o 5f35ea16f836 descr_for_a1 
    |/  
    | o 43fe77b5815e descr_for_b 
    |/  
    o 9929b494c411 trunk1 
    | @ 230dd059e1b0 (no description set) 
    |/  
    o 000000000000 (no description set) 
    "###);
    let stdout = test_env.jj_cmd_success(
        &target_jj_repo_path,
        &["git", "fetch", "--glob", "b", "--glob", "a*"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r###"
    o bae45043dbd2 descr_for_a2 a2
    | o f561acfed1d2 descr_for_b b
    |/  
    | o 51fa81c130b6 descr_for_a1 a1
    |/  
    o c0bc3b78f807 descr_for_trunk2 
    | o cb1fe5d97488 descr_for_a2 
    |/  
    | o 5f35ea16f836 descr_for_a1 
    |/  
    | o 43fe77b5815e descr_for_b 
    |/  
    o 9929b494c411 trunk1 
    | @ 230dd059e1b0 (no description set) 
    |/  
    o 000000000000 (no description set) 
    "###);
}

// TODO: Fix the bug this test demonstrates. The issue likely stems from the
// fact that `jj undo` does not undo the fetch inside the git repo backing the
// `target` repo. It is unclear whether it should.
#[test]
fn test_git_fetch_undo() {
    let test_env = TestEnvironment::default();
    let source_git_repo_path = test_env.env_root().join("source");
    let _git_repo = git2::Repository::init(source_git_repo_path.clone()).unwrap();

    // Clone an empty repo. The target repo is a normal `jj` repo, *not* colocated
    let stdout =
        test_env.jj_cmd_success(test_env.env_root(), &["git", "clone", "source", "target"]);
    insta::assert_snapshot!(stdout, @r###"
    Fetching into new repo in "$TEST_ENV/target"
    Nothing changed.
    "###);
    let target_jj_repo_path = test_env.env_root().join("target");

    let source_log =
        create_colocated_repo_and_branches_from_trunk1(&test_env, &source_git_repo_path);
    insta::assert_snapshot!(source_log, @r###"
       ===== Source git repo contents =====
    @ 43fe77b5815e descr_for_b b
    | o cb1fe5d97488 descr_for_a2 a2
    |/  
    | o 5f35ea16f836 descr_for_a1 a1
    |/  
    o 9929b494c411 trunk1 master trunk1
    o 000000000000 (no description set) 
    "###);

    // Fetch 2 branches
    let stdout = test_env.jj_cmd_success(
        &target_jj_repo_path,
        &["git", "fetch", "--glob", "b", "--glob", "a1"],
    );
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r###"
    o 43fe77b5815e descr_for_b b
    | o 5f35ea16f836 descr_for_a1 a1
    |/  
    o 9929b494c411 trunk1 
    | @ 230dd059e1b0 (no description set) 
    |/  
    o 000000000000 (no description set) 
    "###);
    insta::assert_snapshot!(test_env.jj_cmd_success(&target_jj_repo_path, &["undo"]), @"");
    // The undo works as expected
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r###"
    @ 230dd059e1b0 (no description set) 
    o 000000000000 (no description set) 
    "###);
    // Now try to fetch just one branch
    let stdout = test_env.jj_cmd_success(&target_jj_repo_path, &["git", "fetch", "--glob", "b"]);
    insta::assert_snapshot!(stdout, @"");
    // BUG: Both branches got fetched.
    insta::assert_snapshot!(get_log_output(&test_env, &target_jj_repo_path), @r###"
    o 43fe77b5815e descr_for_b b
    | o 5f35ea16f836 descr_for_a1 a1
    |/  
    o 9929b494c411 trunk1 
    | @ 230dd059e1b0 (no description set) 
    |/  
    o 000000000000 (no description set) 
    "###);
}

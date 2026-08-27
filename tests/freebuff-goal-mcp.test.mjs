import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
const toolsSource = read("../src-tauri/src/chat_model_tools.rs");
const goalSource = read("../src-tauri/src/terminal_goal.rs");
const cargoSource = read("../src-tauri/Cargo.toml");
const providerSource = read("../src-tauri/src/provider.rs");
const serverSource = read("../src-tauri/src/server.rs");
const chatSource = read("../src-tauri/src/chat.rs");
const autonomousSource = read("../src-tauri/src/autonomous.rs");
const readmeSource = read("../README.md");

test("les goals de chat et de terminal ont des scopes distincts", () => {
  assert.match(toolsSource, /GoalsOnly/);
  assert.match(toolsSource, /TERMINAL_GOAL_TOOL_NAMES/);
  assert.match(toolsSource, /Self::GoalsOnly => TERMINAL_GOAL_TOOL_NAMES\.contains/);
  assert.match(
    toolsSource,
    /Self::Full => !\[GET_GOAL_TOOL_NAME, UPDATE_GOAL_TOOL_NAME\]\.contains/,
  );
  assert.match(toolsSource, /goal_tools_response/);
  assert.match(serverSource, /CREATE_GOAL_TOOL_NAME if context\.scope == ChatToolScope::GoalsOnly/);
  assert.match(serverSource, /CREATE_GOAL_TOOL_NAME =>/);
});

test("la configuration MCP Freebuff conserve le bearer hors du disque", () => {
  assert.match(providerSource, /FREEBUFF_GOAL_MCP_SERVER_NAME: &str = "cst_goal"/);
  assert.match(providerSource, /format!\("Bearer \$\{\}"/);
  assert.match(providerSource, /crate::chat_model_tools::MCP_BEARER_ENV/);
  assert.match(serverSource, /goal_tool_lease: Mutex<Option<TerminalGoalToolLease>>/);
  assert.match(serverSource, /registry\.revoke\(&self\.server\.bearer_token\)/);
});

test("le goal terminal est durable et ne peut pas etre ecrase tant qu'il est inacheve", () => {
  assert.match(goalSource, /TerminalGoalManager/);
  assert.match(serverSource, /terminal-goals\.json/);
  assert.match(goalSource, /Un goal Freebuff est deja inacheve/);
  assert.match(goalSource, /TerminalGoalStatus::Active/);
  assert.match(goalSource, /TerminalGoalStatus::Complete/);
  assert.match(goalSource, /TerminalGoalStatus::Blocked/);
  assert.match(goalSource, /self != Self::Complete/);
  assert.match(goalSource, /fs_util::atomic_write/);
  assert.match(goalSource, /self\.persist\(&next_goals\)\?[\s\S]*?\*goals = next_goals/);
  assert.match(serverSource, /terminal_goal_key\(&owner_id, &workspace_id\)/);
});

test("le skill create-goal est installe et visible comme commande TUI Freebuff", () => {
  assert.match(providerSource, /name: switch-create-goal/);
  assert.match(providerSource, /\.join\("\.agents"\)[\s\S]*?\.join\("skills"\)/);
  assert.match(providerSource, /Call `cst_goal__create_goal`/);
  assert.match(providerSource, /Call `cst_goal__get_goal`/);
  assert.match(providerSource, /Call `cst_goal__update_goal`/);
  assert.match(providerSource, /A blocked goal is still unfinished/);
  assert.match(providerSource, /target hundreds to a few thousand generated scenarios/);
  assert.match(providerSource, /If any required test fails, keep the same goal unfinished/);
  assert.match(providerSource, /delete only the temporary proof tests/);
  assert.match(providerSource, /Never delete pre-existing or permanent regression tests/);
  assert.match(providerSource, /FREEBUFF_GOAL_SKILL_DIR/);
  assert.match(readmeSource, /\/skill:switch-create-goal <objectif>/);
});

test("la portee du goal survit aux recreations de terminal et transferts de compte", () => {
  assert.match(goalSource, /terminal_goal_key\(owner_id: &str, workspace_id: &str\)/);
  assert.match(goalSource, /cst-freebuff-goal-scope-v3/);
  assert.match(goalSource, /owner-workspace/);
  assert.match(goalSource, /Sha256::digest\(serialized\)/);
  assert.match(cargoSource, /^sha2 = "0\.10"$/m);
  const serverScope = serverSource.match(
    /let goal_key = terminal_goal_key\(([^;]+)\)\?;/,
  )?.[1] ?? "";
  assert.match(serverScope, /&owner_id/);
  assert.match(serverScope, /&workspace_id/);
  assert.doesNotMatch(serverScope, /account\.id/);
  assert.doesNotMatch(serverScope, /source_terminal_key/);
});

test("les validations create_goal bornent objectifs budgets et portees", () => {
  assert.match(goalSource, /MAX_GOAL_OBJECTIVE_LENGTH: usize = 32_768/);
  assert.match(goalSource, /MAX_PERSISTED_GOALS: usize = 1_024/);
  assert.match(goalSource, /token_budget == Some\(0\)/);
  assert.match(goalSource, /key\.chars\(\)\.count\(\) > 2048/);
  assert.match(goalSource, /workspace_id\.chars\(\)\.any\(char::is_control\)/);
  assert.match(toolsSource, /"objective": \{[\s\S]*?"minLength": 1,[\s\S]*?"maxLength": 32768/);
  assert.match(toolsSource, /"token_budget": \{[\s\S]*?"minimum": 1/);
});

test("les ecritures echouees et les fichiers corrompus ne contaminent pas l'etat", () => {
  assert.match(goalSource, /let mut next_goals = goals\.clone\(\)/);
  assert.match(goalSource, /self\.persist\(&next_goals\)\?/);
  assert.match(goalSource, /with_extension\(format!\([\s\S]*?corrupt-/);
  assert.match(goalSource, /failed_persistence_does_not_mutate_memory/);
  assert.match(goalSource, /corrupt_state_is_quarantined_without_blocking_startup/);
  assert.match(goalSource, /goal_store_is_bounded_and_prunes_the_oldest_completed_goal/);
  assert.match(goalSource, /oldest_completed_key/);
});

test("Claude et OpenCode recoivent create_goal sans exposer le bearer", () => {
  assert.match(chatSource, /mcp__\{MCP_SERVER_NAME\}__\{CREATE_GOAL_TOOL_NAME\}/);
  assert.match(chatSource, /command\.env\(MCP_BEARER_ENV, &config\.bearer_token\)/);
  assert.match(chatSource, /Bearer \{\{env:\{MCP_BEARER_ENV\}\}\}/);
  assert.match(chatSource, /command\.env\(OPENCODE_CONFIG_CONTENT_ENV, document\.to_string\(\)\)/);
  assert.match(chatSource, /`cst_chat_create_goal`/);
});

test("un goal autonome est unique et isole par proprietaire", () => {
  assert.match(serverSource, /manager\.create_goal\(request\)/);
  assert.match(autonomousSource, /pub fn create_goal\(/);
  assert.match(autonomousSource, /candidate\.owner_id == agent\.owner_id/);
  assert.match(autonomousSource, /candidate\.status != AutonomousAgentStatus::Completed/);
  assert.match(toolsSource, /agent\.owner_id == context\.user_id/);
});

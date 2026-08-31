use super::{
    adaptive_fanout_cardinalities, normalize_optional, truncate, validate_required_text,
    validate_short_text, OrchestrationAccountRole, OrchestrationProofTest,
    OrchestrationReviewDecision, OrchestrationSnapshot, OrchestrationTaskStatus,
    OrchestrationTeamMessage, OrchestrationTestDefinition, MAX_TEAM_MESSAGES,
    MAX_TEAM_MESSAGES_PER_TURN, MAX_TEAM_MESSAGE_CHARS, MAX_TEXT_CHARS,
};
use serde::Deserialize;
use std::collections::HashSet;
use uuid::Uuid;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PlanEnvelope {
    pub(super) summary: String,
    pub(super) tasks: Vec<PlanTask>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PlanTask {
    pub(super) title: String,
    pub(super) description: String,
    #[serde(default)]
    pub(super) acceptance_criteria: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TesterPlanEnvelope {
    pub(super) summary: String,
    pub(super) tests: Vec<OrchestrationTestDefinition>,
    #[serde(default)]
    pub(super) messages: Vec<TeamMessageEnvelope>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TesterResultEnvelope {
    pub(super) decision: OrchestrationTesterDecision,
    pub(super) summary: String,
    #[serde(default)]
    pub(super) task_ids: Vec<String>,
    #[serde(default)]
    pub(super) feedback: String,
    pub(super) tests: Vec<OrchestrationProofTest>,
    #[serde(default)]
    pub(super) messages: Vec<TeamMessageEnvelope>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProofEnvelope {
    pub(super) summary: String,
    #[serde(default)]
    pub(super) files_changed: Vec<String>,
    pub(super) tests: Vec<OrchestrationProofTest>,
    #[serde(default)]
    pub(super) risks: Vec<String>,
    #[serde(default)]
    pub(super) messages: Vec<TeamMessageEnvelope>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReviewEnvelope {
    pub(super) decision: OrchestrationReviewDecision,
    pub(super) summary: String,
    #[serde(default)]
    pub(super) feedback: String,
    #[serde(default)]
    pub(super) tests: Vec<OrchestrationProofTest>,
    #[serde(default)]
    pub(super) messages: Vec<TeamMessageEnvelope>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MergeReviewEnvelope {
    pub(super) decision: OrchestrationReviewDecision,
    pub(super) summary: String,
    #[serde(default)]
    pub(super) task_ids: Vec<String>,
    #[serde(default)]
    pub(super) feedback: String,
    #[serde(default)]
    pub(super) tests: Vec<OrchestrationProofTest>,
    #[serde(default)]
    pub(super) messages: Vec<TeamMessageEnvelope>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct FinalEnvelope {
    pub(super) decision: FinalDecision,
    pub(super) summary: String,
    #[serde(default)]
    pub(super) task_id: Option<String>,
    #[serde(default)]
    pub(super) feedback: String,
    #[serde(default)]
    pub(super) tests: Vec<OrchestrationProofTest>,
    #[serde(default)]
    pub(super) messages: Vec<TeamMessageEnvelope>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TeamMessageEnvelope {
    #[serde(default)]
    pub(super) to_task_ids: Vec<String>,
    pub(super) body: String,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum FinalDecision {
    Complete,
    Revise,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum OrchestrationTesterDecision {
    Pass,
    Revise,
}

pub(super) fn parse_marked_json<T: for<'de> Deserialize<'de>>(
    text: &str,
    marker: &str,
) -> Result<T, String> {
    let payload = text
        .lines()
        .rev()
        .find_map(|line| line.trim().strip_prefix(marker).map(str::trim))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("ligne {marker} absente"))?;
    serde_json::from_str(payload).map_err(|error| format!("JSON invalide : {error}"))
}

pub(super) fn validate_plan(
    mut plan: PlanEnvelope,
    run: &OrchestrationSnapshot,
) -> Result<PlanEnvelope, String> {
    plan.summary = validate_short_text(&plan.summary, "Le resume du plan")?;
    if plan.tasks.is_empty() {
        return Err("le plan ne contient aucune tache".to_string());
    }
    let task_count = plan.tasks.len() as u32;
    if run.adaptive_fanout {
        if !adaptive_fanout_cardinalities(run.max_task_count).contains(&task_count) {
            return Err(format!(
                "le plan adaptatif doit utiliser entre 1 et {} taches, mais il en contient {task_count}",
                run.max_task_count
            ));
        }
        if task_count < run.minimum_task_count {
            return Err(format!(
                "le plan contient {task_count} taches, sous le plancher persistant de {}",
                run.minimum_task_count
            ));
        }
    } else if task_count != run.worker_count {
        return Err(format!(
            "le plan doit contenir exactement {} tache{} (une par worker), mais il en contient {}",
            run.worker_count,
            if run.worker_count > 1 { "s" } else { "" },
            plan.tasks.len()
        ));
    }
    let mut signatures = HashSet::new();
    for task in &mut plan.tasks {
        task.title = validate_short_text(&task.title, "Le titre d'une tache")?;
        task.description = validate_short_text(&task.description, "La description d'une tache")?;
        task.acceptance_criteria = task
            .acceptance_criteria
            .drain(..)
            .map(|value| validate_short_text(&value, "Un critere d'acceptation"))
            .collect::<Result<Vec<_>, _>>()?;
        if task.acceptance_criteria.is_empty() {
            return Err(format!("la tache '{}' n'a aucun critere", task.title));
        }
        if task_count >= 20 {
            let signature = format!(
                "{}\n{}",
                task.title.to_lowercase(),
                task.description.to_lowercase()
            );
            if !signatures.insert(signature) {
                return Err(
                    "le plan massif contient des taches dupliquees servant de remplissage"
                        .to_string(),
                );
            }
        }
    }
    Ok(plan)
}

pub(super) fn validate_team_messages(
    messages: &mut Vec<TeamMessageEnvelope>,
    run: &OrchestrationSnapshot,
) -> Result<(), String> {
    if messages.len() > MAX_TEAM_MESSAGES_PER_TURN {
        return Err(format!(
            "un tour ne peut pas publier plus de {MAX_TEAM_MESSAGES_PER_TURN} messages de groupe"
        ));
    }
    let known_tasks = run
        .tasks
        .iter()
        .map(|task| task.id.as_str())
        .collect::<HashSet<_>>();
    for message in messages {
        message.body = validate_required_text(
            &message.body,
            MAX_TEAM_MESSAGE_CHARS,
            "Le message de groupe",
        )?;
        let mut seen = HashSet::new();
        message.to_task_ids = message
            .to_task_ids
            .drain(..)
            .map(|target| target.trim().to_string())
            .filter(|target| !target.is_empty() && seen.insert(target.clone()))
            .collect();
        if let Some(unknown) = message
            .to_task_ids
            .iter()
            .find(|target| !known_tasks.contains(target.as_str()))
        {
            return Err(format!("destinataire de groupe inconnu : {unknown}"));
        }
    }
    Ok(())
}

pub(super) fn append_team_messages(
    run: &mut OrchestrationSnapshot,
    from_role: OrchestrationAccountRole,
    from_task_id: Option<&str>,
    messages: Vec<TeamMessageEnvelope>,
    now: i64,
) {
    let mut sequence = run
        .team_messages
        .last()
        .map(|message| message.sequence.saturating_add(1))
        .unwrap_or(1);
    for message in messages {
        run.team_messages.push(OrchestrationTeamMessage {
            id: Uuid::new_v4().to_string(),
            sequence,
            timestamp: now,
            from_role,
            from_task_id: from_task_id.map(str::to_string),
            to_task_ids: message.to_task_ids,
            body: message.body,
        });
        sequence = sequence.saturating_add(1);
    }
    if run.team_messages.len() > MAX_TEAM_MESSAGES {
        run.team_messages
            .drain(0..run.team_messages.len() - MAX_TEAM_MESSAGES);
    }
}

pub(super) fn validate_proof(mut proof: ProofEnvelope) -> Result<ProofEnvelope, String> {
    proof.summary = validate_short_text(&proof.summary, "Le resume de la preuve")?;
    if proof.tests.is_empty() {
        return Err("aucun test n'est fourni".to_string());
    }
    if proof.tests.iter().any(|test| !test.passed) {
        return Err("au moins un test soumis est en echec".to_string());
    }
    for test in &mut proof.tests {
        test.command = validate_short_text(&test.command, "La commande de test")?;
        test.result = validate_short_text(&test.result, "Le resultat de test")?;
    }
    proof.files_changed = proof
        .files_changed
        .into_iter()
        .filter_map(|value| normalize_optional(Some(value)))
        .collect();
    proof.risks = proof
        .risks
        .into_iter()
        .filter_map(|value| normalize_optional(Some(value)))
        .map(|value| truncate(&value, 2_000))
        .collect();
    Ok(proof)
}

pub(super) fn validate_tester_plan(
    mut plan: TesterPlanEnvelope,
) -> Result<TesterPlanEnvelope, String> {
    plan.summary = validate_short_text(&plan.summary, "Le resume du plan de tests")?;
    if plan.tests.is_empty() {
        return Err("le testeur doit definir au moins un test".to_string());
    }
    for test in &mut plan.tests {
        test.name = validate_short_text(&test.name, "Le nom du test")?;
        test.command = validate_short_text(&test.command, "La commande du test")?;
        test.expected = validate_short_text(&test.expected, "Le resultat attendu")?;
    }
    Ok(plan)
}

pub(super) fn validate_tester_result(
    mut result: TesterResultEnvelope,
    run: &OrchestrationSnapshot,
) -> Result<TesterResultEnvelope, String> {
    result.summary = validate_short_text(&result.summary, "Le resume du testeur")?;
    result.feedback = truncate(result.feedback.trim(), MAX_TEXT_CHARS);
    if result.tests.is_empty() {
        return Err("le testeur doit fournir les resultats des tests".to_string());
    }
    for test in &mut result.tests {
        test.command = validate_short_text(&test.command, "La commande de test")?;
        test.result = validate_short_text(&test.result, "Le resultat de test")?;
    }
    let tester_id = run
        .current_tester_id
        .as_deref()
        .ok_or_else(|| "orchestrateur testeur courant absent".to_string())?;
    let tester = run
        .testers
        .iter()
        .find(|tester| tester.id == tester_id)
        .ok_or_else(|| format!("orchestrateur testeur inconnu : {tester_id}"))?;
    if let Some(missing) = tester.test_plan.iter().find(|planned| {
        !result
            .tests
            .iter()
            .any(|executed| executed.command.trim() == planned.command.trim())
    }) {
        return Err(format!(
            "le test planifie '{}' n'a pas ete execute",
            missing.name
        ));
    }
    let assigned = tester
        .assigned_task_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    result.task_ids = result
        .task_ids
        .drain(..)
        .map(|task_id| task_id.trim().to_string())
        .filter(|task_id| !task_id.is_empty() && seen.insert(task_id.clone()))
        .collect();
    if let Some(unknown) = result
        .task_ids
        .iter()
        .find(|task_id| !assigned.contains(task_id.as_str()))
    {
        return Err(format!(
            "le testeur ne peut rouvrir que ses missions ; taskId interdit : {unknown}"
        ));
    }
    match result.decision {
        OrchestrationTesterDecision::Pass => {
            if result.tests.iter().any(|test| !test.passed) {
                return Err("un testeur ne peut pas valider avec un test en echec".to_string());
            }
            if !result.task_ids.is_empty() || !result.feedback.is_empty() {
                return Err("une validation reussie ne doit pas demander de correction".to_string());
            }
        }
        OrchestrationTesterDecision::Revise => {
            if result.task_ids.is_empty() {
                return Err("une correction doit cibler au moins un worker".to_string());
            }
            if result.feedback.is_empty() {
                return Err("une correction doit fournir un feedback actionnable".to_string());
            }
        }
    }
    Ok(result)
}

pub(super) fn validate_review(mut review: ReviewEnvelope) -> Result<ReviewEnvelope, String> {
    review.summary = validate_short_text(&review.summary, "Le resume de la revue")?;
    review.feedback = truncate(review.feedback.trim(), MAX_TEXT_CHARS);
    if review.decision == OrchestrationReviewDecision::Accept {
        if review.tests.is_empty() {
            return Err("une acceptation sans test est interdite".to_string());
        }
        if review.tests.iter().any(|test| !test.passed) {
            return Err("une acceptation contient un test en echec".to_string());
        }
    } else if review.feedback.is_empty() {
        return Err("une revision doit contenir un feedback".to_string());
    }
    Ok(review)
}

pub(super) fn validate_merge_review(
    mut review: MergeReviewEnvelope,
    run: &OrchestrationSnapshot,
    pending_task_ids: &[String],
) -> Result<MergeReviewEnvelope, String> {
    review.summary = validate_short_text(&review.summary, "Le resume de la fusion")?;
    review.feedback = truncate(review.feedback.trim(), MAX_TEXT_CHARS);
    if review.tests.is_empty() {
        return Err("la revue de fusion exige au moins un test execute".to_string());
    }
    for test in &mut review.tests {
        test.command = validate_short_text(&test.command, "La commande de fusion")?;
        test.result = validate_short_text(&test.result, "Le resultat de fusion")?;
    }

    let accepted = run
        .tasks
        .iter()
        .filter(|task| task.status == OrchestrationTaskStatus::Accepted)
        .map(|task| task.id.as_str())
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    review.task_ids = review
        .task_ids
        .drain(..)
        .map(|task_id| task_id.trim().to_string())
        .filter(|task_id| !task_id.is_empty() && seen.insert(task_id.clone()))
        .collect();
    if let Some(unknown) = review
        .task_ids
        .iter()
        .find(|task_id| !accepted.contains(task_id.as_str()))
    {
        return Err(format!(
            "la fusion ne peut rouvrir qu'une mission acceptee ; taskId interdit : {unknown}"
        ));
    }

    match review.decision {
        OrchestrationReviewDecision::Accept => {
            if review.tests.iter().any(|test| !test.passed) {
                return Err("une fusion acceptee contient un test en echec".to_string());
            }
            if !review.task_ids.is_empty() || !review.feedback.is_empty() {
                return Err("une fusion acceptee ne doit pas demander de correction".to_string());
            }
            if pending_task_ids.is_empty() {
                return Err("aucune mission validee n'attend la fusion".to_string());
            }
        }
        OrchestrationReviewDecision::Revise => {
            if review.task_ids.is_empty() {
                return Err("une correction de fusion doit cibler au moins un worker".to_string());
            }
            if review.feedback.is_empty() {
                return Err(
                    "une correction de fusion doit fournir un feedback actionnable".to_string(),
                );
            }
        }
    }
    Ok(review)
}

pub(super) fn validate_final(
    mut final_review: FinalEnvelope,
    run: &OrchestrationSnapshot,
) -> Result<FinalEnvelope, String> {
    final_review.summary = validate_short_text(&final_review.summary, "Le resume final")?;
    final_review.feedback = truncate(final_review.feedback.trim(), MAX_TEXT_CHARS);
    match final_review.decision {
        FinalDecision::Complete => {
            if final_review.tests.is_empty() || final_review.tests.iter().any(|test| !test.passed) {
                return Err("la conclusion finale exige au moins un test reussi".to_string());
            }
            final_review.task_id = None;
        }
        FinalDecision::Revise => {
            let task_id = final_review
                .task_id
                .as_deref()
                .ok_or_else(|| "taskId absent pour la revision".to_string())?;
            if !run.tasks.iter().any(|task| task.id == task_id) {
                return Err(format!("taskId inconnu : {task_id}"));
            }
            if final_review.feedback.is_empty() {
                return Err("le feedback de revision est vide".to_string());
            }
        }
    }
    Ok(final_review)
}

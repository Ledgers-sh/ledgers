//! Command implementations for policy operations: `put_key`, `revoke_key`, `explain`.

use crate::surface::{
    PolicyExplainInput, PolicyExplainOutput, PutKeyInput, PutKeyOutput, RevokeKeyInput,
    RevokeKeyOutput,
};
use cratefield_core::Database;
use ledgers_commands::{
    Command, CommandContext, CommandError, ExecToken, JournalPreview, Plan, Tier,
};

/// `policy.put_key`: create or update an agent key.
pub struct PutKeyCommand;

#[async_trait::async_trait]
impl Command for PutKeyCommand {
    const NAME: &'static str = "policy.put_key";
    const TIER: Tier = Tier::PostWithinLimits;
    const HUMAN_ONLY: bool = true;
    const SUPPORTS_DRY_RUN: bool = false;
    const IDEMPOTENT: bool = false;

    type Input = PutKeyInput;
    type Output = PutKeyOutput;

    async fn preview(
        &self,
        _db: &dyn Database,
        _ctx: &CommandContext,
        _input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        // Policy operations move no money.
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        _db: &dyn Database,
        _ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        // Stub: actual implementation would validate, upsert the key, and audit it.
        Ok(Plan::with_output(PutKeyOutput {
            key_id: input.key_id.clone(),
            message: format!("key {} would be created/updated", input.key_id),
        }))
    }
}

/// `policy.revoke_key`: mark an agent key as revoked.
pub struct RevokeKeyCommand;

#[async_trait::async_trait]
impl Command for RevokeKeyCommand {
    const NAME: &'static str = "policy.revoke_key";
    const TIER: Tier = Tier::PostWithinLimits;
    const HUMAN_ONLY: bool = true;
    const SUPPORTS_DRY_RUN: bool = false;
    const IDEMPOTENT: bool = false;

    type Input = RevokeKeyInput;
    type Output = RevokeKeyOutput;

    async fn preview(
        &self,
        _db: &dyn Database,
        _ctx: &CommandContext,
        _input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        // Policy operations move no money.
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        _db: &dyn Database,
        _ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        // Stub: actual implementation would mark the key as revoked and audit it.
        Ok(Plan::with_output(RevokeKeyOutput {
            key_id: input.key_id.clone(),
            message: format!("key {} would be revoked", input.key_id),
        }))
    }
}

/// `policy.explain`: get a decision explanation for a command.
pub struct PolicyExplainCommand;

#[async_trait::async_trait]
impl Command for PolicyExplainCommand {
    const NAME: &'static str = "policy.explain";
    const TIER: Tier = Tier::Read;
    const HUMAN_ONLY: bool = false;
    const SUPPORTS_DRY_RUN: bool = false;
    const IDEMPOTENT: bool = true;

    type Input = PolicyExplainInput;
    type Output = PolicyExplainOutput;

    async fn preview(
        &self,
        _db: &dyn Database,
        _ctx: &CommandContext,
        _input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        // Read-only operation.
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        _db: &dyn Database,
        _ctx: &CommandContext,
        _input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        // Stub: actual implementation would look up the key and command, then call explain().
        Ok(Plan::with_output(PolicyExplainOutput {
            decision: "Allow".to_string(),
            class: "Read".to_string(),
            steps: vec!["Stub implementation".to_string()],
            reason: None,
        }))
    }
}

/// Builds the policy registry: returns all policy commands.
#[must_use]
pub fn registry() -> ledgers_commands::Registry {
    let mut reg = ledgers_commands::Registry::new();

    // Register policy.put_key
    let input_schema =
        serde_json::to_value(schemars::schema_for!(PutKeyInput)).unwrap_or(serde_json::json!({}));
    let output_schema =
        serde_json::to_value(schemars::schema_for!(PutKeyOutput)).unwrap_or(serde_json::json!({}));
    if let Ok(descriptor) = ledgers_commands::CommandDescriptor::from_command::<PutKeyCommand>(
        input_schema,
        output_schema,
    ) {
        let _ = reg.register(descriptor);
    }

    // Register policy.revoke_key
    let input_schema = serde_json::to_value(schemars::schema_for!(RevokeKeyInput))
        .unwrap_or(serde_json::json!({}));
    let output_schema = serde_json::to_value(schemars::schema_for!(RevokeKeyOutput))
        .unwrap_or(serde_json::json!({}));
    if let Ok(descriptor) = ledgers_commands::CommandDescriptor::from_command::<RevokeKeyCommand>(
        input_schema,
        output_schema,
    ) {
        let _ = reg.register(descriptor);
    }

    // Register policy.explain
    let input_schema = serde_json::to_value(schemars::schema_for!(PolicyExplainInput))
        .unwrap_or(serde_json::json!({}));
    let output_schema = serde_json::to_value(schemars::schema_for!(PolicyExplainOutput))
        .unwrap_or(serde_json::json!({}));
    if let Ok(descriptor) = ledgers_commands::CommandDescriptor::from_command::<PolicyExplainCommand>(
        input_schema,
        output_schema,
    ) {
        let _ = reg.register(descriptor);
    }

    reg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_policy_commands_registered() {
        let reg = registry();
        assert!(reg.get("policy.put_key").is_some());
        assert!(reg.get("policy.revoke_key").is_some());
        assert!(reg.get("policy.explain").is_some());
    }

    #[test]
    fn test_policy_commands_metadata() {
        let reg = registry();

        // put_key and revoke_key must be human_only
        if let Some(put_key) = reg.get("policy.put_key") {
            assert!(put_key.human_only, "policy.put_key must be human_only");
        }

        if let Some(revoke_key) = reg.get("policy.revoke_key") {
            assert!(
                revoke_key.human_only,
                "policy.revoke_key must be human_only"
            );
        }

        // explain can be used by agents
        if let Some(explain) = reg.get("policy.explain") {
            assert!(
                !explain.human_only,
                "policy.explain should be callable by agents"
            );
        }
    }
}

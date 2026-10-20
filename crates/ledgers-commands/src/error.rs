//! [`CommandError`] and `ProblemDef` definitions for the command executor.
//!
//! Every error a command can produce converts to an RFC 9457 Problem.
//! Problem type URIs follow `https://ledgers.sh/problems/<slug>`.

use cratefield_core::Problem;

/// Every way a command can fail.
#[derive(Debug, Clone, thiserror::Error)]
pub enum CommandError {
    /// The credential did not authenticate: the principal could not be
    /// determined.
    #[error("unauthenticated")]
    Unauthenticated,

    /// The principal's tier is below the command's required tier.
    #[error("tier insufficient: required {required}, got {actual}")]
    TierInsufficient {
        /// The required tier.
        required: String,
        /// The actual tier.
        actual: String,
    },

    /// The command is human-only, but the principal is an agent.
    #[error("command is human-only")]
    HumanOnly,

    /// The policy engine denied the command (no reason returned to avoid
    /// timing attacks on limits).
    #[error("policy denied")]
    PolicyDenied,

    /// The command does not support dry-run, but dry-run was requested.
    #[error("dry-run not supported by this command")]
    DryRunUnsupported,

    /// The command's input is malformed (failed to deserialize or validate).
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// The batch still conflicted after every retry attempt.
    #[error("posting still conflicts after {attempts} attempts")]
    Conflict {
        /// The attempts the policy allowed.
        attempts: u32,
    },

    /// A database error (query, constraint, etc.).
    #[error(transparent)]
    Database(#[from] cratefield_core::DbError),

    /// A domain error from another crate (e.g., ledgers-ledger, ledgers-audit).
    /// The problem is carried through unchanged.
    #[error(transparent)]
    Domain(#[from] cratefield_core::Problem),
}

impl From<ledgers_db::GuardError> for CommandError {
    fn from(err: ledgers_db::GuardError) -> Self {
        match err {
            ledgers_db::GuardError::Conflict { attempts } => CommandError::Conflict { attempts },
            ledgers_db::GuardError::Database(db) => CommandError::Database(db),
        }
    }
}

impl From<ledgers_money::MoneyError> for CommandError {
    fn from(err: ledgers_money::MoneyError) -> Self {
        CommandError::InvalidInput(format!("money error: {err}"))
    }
}

impl CommandError {
    /// Converts this error to an RFC 9457 Problem.
    #[must_use]
    pub fn to_problem(&self) -> Problem {
        match self {
            CommandError::Unauthenticated => {
                Problem::new(&UNAUTHENTICATED).with_detail("credential not recognized")
            }
            CommandError::TierInsufficient { required, actual } => Problem::new(&TIER_INSUFFICIENT)
                .with_detail(format!(
                    "this command requires tier {required}, you have {actual}"
                ))
                .with_extension("requiredTier", required.clone())
                .with_extension("actualTier", actual.clone()),
            CommandError::HumanOnly => {
                Problem::new(&HUMAN_ONLY).with_detail("this command is human-only")
            }
            CommandError::PolicyDenied => {
                Problem::new(&POLICY_DENIED).with_detail("policy decision is to deny")
            }
            CommandError::DryRunUnsupported => Problem::new(&DRY_RUN_UNSUPPORTED)
                .with_detail("this command does not support --dry-run"),
            CommandError::InvalidInput(msg) => Problem::new(&INVALID_INPUT).with_detail(msg),
            CommandError::Conflict { attempts } => {
                #[allow(clippy::cast_possible_wrap)]
                let attempts_i32 = *attempts as i32;
                Problem::new(&CONFLICT)
                    .with_detail(format!("posting conflicted after {attempts} attempts"))
                    .with_extension("attempts", attempts_i32)
            }
            CommandError::Database(err) => {
                Problem::new(&DATABASE_ERROR).with_detail(err.to_string())
            }
            CommandError::Domain(problem) => problem.clone(),
        }
    }
}

/// Unauthenticated: 401
pub const UNAUTHENTICATED: cratefield_core::ProblemDef = cratefield_core::ProblemDef {
    slug: "unauthenticated",
    status: cratefield_core::axum::http::StatusCode::UNAUTHORIZED,
    title: "Unauthenticated",
    description: "The credential did not authenticate.",
};

/// Tier insufficient: 403
pub const TIER_INSUFFICIENT: cratefield_core::ProblemDef = cratefield_core::ProblemDef {
    slug: "tier-insufficient",
    status: cratefield_core::axum::http::StatusCode::FORBIDDEN,
    title: "Tier insufficient",
    description: "The principal's tier is below the command's required tier.",
};

/// Human-only: 403
pub const HUMAN_ONLY: cratefield_core::ProblemDef = cratefield_core::ProblemDef {
    slug: "human-only",
    status: cratefield_core::axum::http::StatusCode::FORBIDDEN,
    title: "Human-only command",
    description: "This command can only be run by a human, not an agent.",
};

/// Policy denied: 403
pub const POLICY_DENIED: cratefield_core::ProblemDef = cratefield_core::ProblemDef {
    slug: "policy-denied",
    status: cratefield_core::axum::http::StatusCode::FORBIDDEN,
    title: "Policy denied",
    description: "The policy engine denied this command.",
};

/// Dry-run not supported: 400
pub const DRY_RUN_UNSUPPORTED: cratefield_core::ProblemDef = cratefield_core::ProblemDef {
    slug: "dry-run-unsupported",
    status: cratefield_core::axum::http::StatusCode::BAD_REQUEST,
    title: "Dry-run not supported",
    description: "This command does not support dry-run mode.",
};

/// Invalid input: 422
pub const INVALID_INPUT: cratefield_core::ProblemDef = cratefield_core::ProblemDef {
    slug: "invalid-input",
    status: cratefield_core::axum::http::StatusCode::UNPROCESSABLE_ENTITY,
    title: "Invalid input",
    description: "The command input is malformed or invalid.",
};

/// Conflict: 409
pub const CONFLICT: cratefield_core::ProblemDef = cratefield_core::ProblemDef {
    slug: "conflict",
    status: cratefield_core::axum::http::StatusCode::CONFLICT,
    title: "Conflict",
    description: "The operation conflicted after retries.",
};

/// Database error: 500
pub const DATABASE_ERROR: cratefield_core::ProblemDef = cratefield_core::ProblemDef {
    slug: "database-error",
    status: cratefield_core::axum::http::StatusCode::INTERNAL_SERVER_ERROR,
    title: "Database error",
    description: "A database operation failed.",
};

/// The base URI for this crate's problem types under RFC 9457.
pub const PROBLEM_BASE: &str = "https://ledgers.sh/problems/";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_errors_convert_to_problems() {
        let test_cases = vec![
            CommandError::Unauthenticated,
            CommandError::TierInsufficient {
                required: "draft".to_string(),
                actual: "read".to_string(),
            },
            CommandError::HumanOnly,
            CommandError::PolicyDenied,
            CommandError::DryRunUnsupported,
            CommandError::InvalidInput("test".to_string()),
            CommandError::Conflict { attempts: 1 },
        ];

        for err in test_cases {
            let _problem = err.to_problem();
        }
    }

    #[test]
    fn command_error_to_problem() {
        let err = CommandError::Unauthenticated;
        let problem = err.to_problem();
        assert_eq!(problem.slug, "unauthenticated");

        let err = CommandError::HumanOnly;
        let problem = err.to_problem();
        assert_eq!(problem.slug, "human-only");

        let err = CommandError::DryRunUnsupported;
        let problem = err.to_problem();
        assert_eq!(problem.slug, "dry-run-unsupported");
    }

    #[test]
    fn problem_base_constant() {
        assert_eq!(PROBLEM_BASE, "https://ledgers.sh/problems/");
    }
}

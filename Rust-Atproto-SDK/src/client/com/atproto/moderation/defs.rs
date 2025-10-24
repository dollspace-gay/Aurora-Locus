//! Generated type definitions for com.atproto.moderation.defs

use serde::{Deserialize, Serialize};

/// Spam: frequent unwanted promotion, replies, mentions. Prefer new lexicon definition `tools.ozone.report.defs#reasonMisleadingSpam`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonSpam;

/// Direct violation of server rules, laws, terms of service. Prefer new lexicon definition `tools.ozone.report.defs#reasonRuleOther`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonViolation;

/// Unwanted or mislabeled sexual content. Prefer new lexicon definition `tools.ozone.report.defs#reasonSexualUnlabeled`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonSexual;

/// Rude, harassing, explicit, or otherwise unwelcoming behavior. Prefer new lexicon definition `tools.ozone.report.defs#reasonHarassmentOther`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonRude;

/// Reports not falling under another report category. Prefer new lexicon definition `tools.ozone.report.defs#reasonOther`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonOther;

pub type ReasonType = String;

/// Appeal a previously taken moderation action
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonAppeal;

/// Tag describing a type of subject that might be reported.
pub type SubjectType = String;

/// Misleading identity, affiliation, or content. Prefer new lexicon definition `tools.ozone.report.defs#reasonMisleadingOther`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonMisleading;


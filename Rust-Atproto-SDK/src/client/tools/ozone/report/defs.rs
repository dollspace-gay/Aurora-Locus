//! Generated type definitions for tools.ozone.report.defs

use serde::{Deserialize, Serialize};

/// Other misleading content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonMisleadingOther;

/// Animal welfare violations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonViolenceAnimal;

/// False information about elections
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonMisleadingElections;

/// Privacy violation involving a minor
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonChildSafetyPrivacy;

/// Other harassing or hateful content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonHarassmentOther;

/// Non-consensual intimate imagery
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonSexualNcii;

/// Promoting or selling prohibited items or services
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonRuleProhibitedSales;

/// Hate speech
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonHarassmentHateSpeech;

/// Human trafficking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonViolenceTrafficking;

/// Unlabelled adult content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonSexualUnlabeled;

/// Other sexual violence content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonSexualOther;

pub type ReasonType = String;

/// Other violent content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonViolenceOther;

/// Graphic violent content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonViolenceGraphicContent;

/// Scam
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonMisleadingScam;

/// Hacking or system attacks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonRuleSiteSecurity;

/// Grooming or predatory behavior. These reports will be sent only be sent to the application's Moderation Authority.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonChildSafetyGroom;

/// Deepfake adult content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonSexualDeepfake;

/// Other
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonRuleOther;

/// Glorification of violence
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonViolenceGlorification;

/// Adult sexual abuse content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonSexualAbuseContent;

/// Dangerous substances or drug abuse
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonSelfHarmSubstances;

/// Other dangerous content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonSelfHarmOther;

/// Other child safety. These reports will be sent only be sent to the application's Moderation Authority.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonChildSafetyOther;

/// Animal sexual abuse
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonSexualAnimal;

/// Child sexual abuse material (CSAM). These reports will be sent only be sent to the application's Moderation Authority.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonChildSafetyCsam;

/// Harassment or bullying of minors
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonChildSafetyHarassment;

/// Threats or incitement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonViolenceThreats;

/// Doxxing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonHarassmentDoxxing;

/// Extremist content. These reports will be sent only be sent to the application's Moderation Authority.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonViolenceExtremistContent;

/// Fake account or bot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonMisleadingBot;

/// Trolling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonHarassmentTroll;

/// An issue not included in these options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonOther;

/// Dangerous challenges or activities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonSelfHarmStunts;

/// Targeted harassment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonHarassmentTargeted;

/// Eating disorders
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonSelfHarmEd;

/// Appeal a previously taken moderation action
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonAppeal;

/// Banned user returning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonRuleBanEvasion;

/// Spam
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonMisleadingSpam;

/// Impersonation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonMisleadingImpersonation;

/// Content promoting or depicting self-harm
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonSelfHarmContent;


// `CalimeroClient` and the helpers that go with it: the top layer. The client
// exposes the generated method groups (`client.orgs.get({ org })`, built in
// routes/ from generated/) on one runtime transport, and adds what no single
// route is: the passkey ceremonies (`signUp`, `signIn`, `addPasskey`, account
// recovery), downloads as bytes, the event streams, and the consent-page and
// approval flows (`approvals.ts`), with organization recovery keys
// (`recovery-key.ts`) and capability bits (`capabilities.ts`). It re-exports
// the runtime's errors, passkeys and directory, which callers of the client
// use. src/routes/index.ts describes the layering.

export { CalimeroClient, FILE_PART_BYTES, deployerKey, frontendManifest, pageDeployment, presenceState } from './calimero-client.js';
export { ExportError } from './files.js';
export { jobKeyBinding, receiptMessage, reportMessage, verifyAppCluster, verifyJobReports, verifyPipelineReceipt, verifyReceiptKey } from './receipts.js';
export type { AttestedCluster, CachedRun, FileRef, JobAttestationDoc, JobEvidence, JobOutput, PipelineReceipt, ReportFields, StepReceipt, StepRunProof, ReceiptAnswer, ReceiptKeyDoc } from './receipts.js';
export type { ExportErrorCode, ExportedFile, FileSink } from './files.js';
export { DEFAULT_DIRECTORY, DirectoryError } from '../runtime/directory.js';
export type { DirectoryFailure } from '../runtime/directory.js';
export {
  AppError,
  ArgsTooLarge,
  BadOrigin,
  CalimeroError,
  ClusterNotUpgraded,
  ConfirmCancelled,
  ConfirmPromptBlocked,
  ConfirmRequired,
  IdempotencyMismatch,
  LoginRequired,
  ConsentStale,
  FeatureNotActive,
  NotFound,
  NotServing,
  OriginNotRegistered,
  OriginTaken,
  PasskeyUnknownHere,
  QuotaExceeded,
  RecoveryNotReady,
  ReleaseDeadlineError,
  RuntimeTooOld,
  SignUpRefused,
  TrapError,
  UnavailableError,
  WrongNode,
  WrongRecoveryCode,
  errorFrom,
} from '../runtime/errors.js';
export type { Committed, SignUpRefusal } from '../runtime/errors.js';
export type {
  AppInfo,
  AppTeam,
  AppTeams,
  BlobInfo,
  FileData,
  FileInfo,
  DeadLetter,
  Deployer,
  FrontendManifest,
  PackageOrigin,
  SignInOptions,
  AccountRecovery,
  SignUpOptions,
  SignUpPolicy,
  CallResult,
  CalimeroClientOptions,
  CalimeroConnectOptions,
  NodeEvent,
  CalimeroSession,
  PackageInfo,
  RetryOptions,
  OrgInfo,
  OrgInvite,
  OrgMode,
  RecoveryTimes,
  PresenceEntry,
  ScopeAppsResult,
  ScopedAppTeam,
  TeamInfo,
  UpgradeRun,
  ViewResult,
} from './calimero-client.js';
export type { AppAbi, AppAbiMethod, AppMethodKind, PackageCapabilities } from './package-capabilities.js';
export { claimedRelease, fetchAttestedKey, freshCollateral, MOCK_QUOTE_HEADER, verifyNode } from '../runtime/attestation.js';
export type { AttestationDoc, NodePolicy, NodeReleasePolicy, NodeVerification } from '../runtime/attestation.js';
export { browserPasskey, b64url, fromB64url, passkeyConfirm } from '../runtime/passkey.js';
export {
  MAX_RECOVERY_NONCE_BYTES,
  RECOVERY_DOMAIN,
  newRecoverySeed,
  parseRecoverySeed,
  recoveryMessage,
  recoveryPublicKey,
  recoverySeedText,
  signRecovery,
} from './recovery-key.js';
export type {
  ConfirmCallback,
  ConfirmChallenge,
  PasskeyConfirmOptions,
  AssertionJSON,
  PasskeyCreateOptions,
  PasskeyGetOptions,
  PasskeyProvider,
  PasskeyUserVerification,
  RegistrationJSON,
} from '../runtime/passkey.js';
export {
  CONSENT_STORAGE_KEY,
  MAX_REPORT_PAGE,
  RELEASE_VERIFIER,
  acceptInviteConsent,
  compareVersions,
  confirmationCode,
  consentReturn,
  createInviteConsent,
  createOrgConsent,
  deleteOrgConsent,
  leaveOrgConsent,
  removeMemberConsent,
  removeMemberTeamsConsent,
  renameOrgConsent,
  revokeInviteConsent,
  setAllAppsConsent,
  setCapabilitiesConsent,
  setMemberRoleConsent,
  setMemberTeamsConsent,
  pageQuery,
  pendingApprovalsFrom,
  publisherBindingBytes,
  publisherBindingMessage,
  releaseConsent,
  releaseNotesUrl,
  stalledBy,
  upgradeConsent,
  verifyNodeUrl,
  verifyReleaseUrl,
  withdrawConsent,
  withoutConsentReturn,
} from './approvals.js';
export type {
  ActorKind,
  AppsReport,
  Authority,
  CapabilitiesConsentSubject,
  ConsentKind,
  ConsentOutcomes,
  ConsentRequest,
  ConsentResult,
  ConsentResultOf,
  ConsentScope,
  ConsentStarted,
  InviteConsentSubject,
  InviteRole,
  MemberTeamConsent,
  OrgRole,
  TeamRole,
  OpenConsentOptions,
  ResumeConsentOptions,
  Governance,
  LatestBlocked,
  PageOptions,
  PendingApprovals,
  PendingRelease,
  PendingUpgrade,
  ReleaseDeadline,
  ReleaseRules,
  ReportScope,
  ReportedApp,
  ReportedBundle,
  RuntimeShortNode,
} from './approvals.js';
export { DEFAULT_ORG_CAPABILITIES, ORG_CAPABILITIES, orgCapabilities } from './capabilities.js';
export type { OrgCapability } from './capabilities.js';

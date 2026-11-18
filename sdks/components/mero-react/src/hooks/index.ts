export { useMero } from '../context';
export { NotConnectedError, NoAppError } from './internal';
export type { MutationOptions, AsyncState, ResourceState } from './internal';

export { useApp, useExecute, useView, useAppAbi } from './app';
export type { CallOptions, ViewOptions, UseViewOptions, UseViewResult } from './app';

export { useSubscription, followEvents } from './events';
// eslint-disable-next-line @typescript-eslint/no-deprecated -- mero-react's own 0.x alias, kept for compatibility
export type { AppEvent, SubscriptionEventData, SubscriptionOptions } from './events';

export { useApps, useCreateApp, useDeleteApp, useSetAppMetadata, useUpgradeApp, usePackages } from './apps';
export type { CreateAppInput, UpgradeTarget } from './apps';

export {
  useOrg,
  useCreateOrg,
  useDeleteOrg,
  useSetMember,
  useRemoveMember,
  useSetOrgMetadata,
  useSetMemberMetadata,
  useSetCapabilities,
  useTeam,
  useCreateTeam,
  useUpdateTeam,
  useDeleteTeam,
  useSetTeamMember,
  useRemoveTeamMember,
} from './orgs';
export type { TeamJoin, CreateTeamInput, TeamUpdate } from './orgs';

export { useCreateInvite, useRedeemInvite, useInviteRedemption, inviteLink, readInviteToken } from './invites';
export type { CreateInviteInput, InviteRedemption, InviteRedemptionStatus } from './invites';

export { useConsent } from './consent';
export type { WaitOptions } from './consent';

export { useUploadBlob, useBlobInfo, useBlobUrl, useBlobs, useDeleteBlob } from './blobs';
export type { BlobHookOptions, UseBlobUrlOptions } from './blobs';

export { useEphemeral, jsonCodec } from './presence';
export type { Codec, EphemeralEntry, UseEphemeralOptions, UseEphemeralResult } from './presence';

export { useAccount, usePasskeys, useApiKeys } from './account';
export type { CreateApiKeyInput } from './account';

import { FinalExecutionStatus, FinalExecutionStatusBasic } from "near-api-js/lib/providers";

export function getWalletCallbackUrl(): string {
  return window.location.origin + "/admin/confirm-wallet";
}

export function isFinalExecutionStatus(status: FinalExecutionStatus | FinalExecutionStatusBasic): status is FinalExecutionStatus {
  return (status as FinalExecutionStatus).SuccessValue !== undefined || (status as FinalExecutionStatus).Failure !== undefined;
}

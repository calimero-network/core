import { FinalExecutionStatus } from "near-api-js/lib/providers";
import * as nearAPI from "near-api-js";

export function getWalletCallbackUrl(): string {
  return window.location.origin + "/admin/confirm-wallet";
}

export function isFinalExecution(response: nearAPI.providers.FinalExecutionOutcome | void): boolean {
  return (response?.status as FinalExecutionStatus).SuccessValue !== undefined || (response?.status as FinalExecutionStatus).Failure !== undefined;
}

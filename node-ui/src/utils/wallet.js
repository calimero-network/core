export function getWalletCallbackUrl() {
  if (!window.location) throw new Error("Window location not available");
  return window.location.origin + "/admin/confirm-wallet";
}

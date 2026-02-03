# Phishing email URL analysis

Target URL:
https://ctrk.klclick3.com/l/01KGFXH325R0DHFE6CD1W7QJPY_0

## Summary
- The link is a Klaviyo click-tracking URL (klclick3.com, ctrk subdomain).
- Its normal behavior is to record a click and then redirect to the final
  destination configured in the email campaign.
- In this environment the link returns HTTP 404 with no Location header and a
  Klaviyo-branded "Oops Error 404" page, so the redirect target cannot be
  recovered from the tracking URL alone.

## What the link is designed to do
1. Log the click (IP, user agent, timestamp) for the sender's campaign metrics.
2. Potentially set tracking cookies for attribution.
3. Issue an HTTP redirect (typically 302/307) to the real destination.

Because the tracking domain is separate from the destination, the final URL is
hidden from the recipient until the redirect occurs. Phishers can abuse this by
placing a malicious destination behind a legitimate tracking domain.

## Observed behavior (2026-02-03)
- GET request returned HTTP/2 404.
- Response headers indicate Cloudflare in front of Klaviyo infrastructure.
- Response body is a Klaviyo 404 page with no meta-refresh or script redirect.

## Implications
- The link itself is a relay and does not host content.
- The campaign link appears to be expired, removed, or malformed.
- The actual phishing destination (if any) cannot be confirmed from this URL.

## Recommended next steps
- If the full email source is available, extract the original href and inspect
  for any encoded destination parameters.
- Use a sandboxed environment to capture the full redirect chain if the link
  becomes active again.
- Treat the campaign as suspicious and monitor for similar tracking domains in
  mail filters and user reports.

## HTML payload analysis (provided snippet)
The supplied HTML appears to be a full Gmail web app shell for a Google
Workspace tenant ("Calimero Network Mail"). It is the standard Gmail loading
page and error/offline templates, not a credential-harvesting form.

Key indicators:
- Title and app name: "Calimero Network Mail".
- Canonical and app URLs point to https://mail.google.com/mail/.
- Large Gmail bootstrap config (GM_BOOTSTRAP_DATA, GM_JS_URLS) and scripts
  loaded from mail.google.com and gstatic.com.
- Embedded data iframe pointing at:
  https://mail.google.com/mail/u/1/data?...
- Error and offline HTML templates embedded in JS (GM_writeErrorPage).
- No form posts to non-Google domains and no obvious credential capture fields.

What it actually does:
- Loads Gmail's JS bundles and CSS from Google-controlled domains.
- Sets/reads cookies for Gmail session and feature gating.
- Creates an iframe to load Gmail data endpoints.
- Includes Google telemetry/error reporting and CSP/prototype tamper checks.

Implications:
- On its own, this HTML does not exfiltrate credentials or redirect to a
  third-party host. It is consistent with a legitimate Gmail web app shell.
- If this HTML was delivered from a non-Google domain, it would still try to
  pull assets from mail.google.com and likely fail or show error pages unless
  the user is already authenticated to Google in that browser.

If this appeared in a phishing email:
- The phishing risk would be in the delivery context (a deceptive link or
  attachment), not in the HTML itself.
- Ask for the full email headers and the exact URL hosting the HTML to confirm
  the origin.

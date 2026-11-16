### Fixed

- **An app's error is shown** (`calimero-client`, so `meroctl`): `meroctl call`
  printed only `422 APP_ERROR: the app refused the call`. The client's error text
  now ends with what the node's `details` says went wrong: the app's error code and
  fields (`422 APP_ERROR: the app refused the call (NotFound: {"key":"a"})`), a
  trap's kind (`422 TRAP`) and the error a failed `init` ended with
  (`422 INIT_FAILED`). Other codes' details, meant for the program, stay out of it.

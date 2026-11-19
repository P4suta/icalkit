# Conformance subject

`icalkit-conformance` is an unpublished test corpus and process-isolated subject for icalkit.
Its stable boundary is newline-delimited JSON on stdin/stdout, so another implementation can drive or reproduce a case without depending on workspace internals.

The package also has an unpublished helper library target for its own white-box adversarial tests.
The helper target compiles icalkit's private module tree as shared source in an isolated crate root.
The helper target has no consumer API or semantic versioning contract.
Runtime subject operations use only the public `icalkit` API.

Run the subject from the workspace with `mise x -- cargo run --locked -p icalkit-conformance --bin icalkit-conformance`.
Protocol version `icalkit-conformance/1` accepts one object per line.
An empty input is a negative probe:

```json
{"protocol":"icalkit-conformance/1","id":"case-1","operation":"strict-parse","input_hex":""}
```

The probe returns a `rejected` outcome with code `icalkit.validation.failed`.
Supported operations are `strict-parse`, `normalize-rfc-repair-v1`, and `normalize-common-clients-v1`.
Every response repeats the protocol, reports an `outcome`, and uses stable string codes for errors, issues, and normalization changes.
The response echoes `id` when the subject can read or recover it from the request.
Malformed JSON in a valid UTF-8 line yields a `protocol-error` response, and the process continues.
Invalid UTF-8 input ends the process unsuccessfully without a response for that line.

The test client sends and flushes one request, then reads its response before sending another.
Closing input after the last request signals completion, and the client waits for a successful subject exit.
Missing, malformed, and extra responses fail the exchange.
The client also stops and waits for an unfinished subject during cleanup.
Cleanup runs on early returns and unwinding.

The versioned corpus manifest is `corpus/manifest.v1.jsonl`.
The manifest labels existing client-shaped cases `synthetic`.
Synthetic cases provide no evidence for a compatibility repair.
A captured Google Calendar, Microsoft 365, or Apple Calendar row must record its version and observation date and attest its reduction and anonymization before it can justify `CommonClientsV1` behavior.

Raw producer material enters through the non-writing `capture-audit` maintainer CLI and remains outside the workspace.
[CAPTURE.md](CAPTURE.md) describes the exact daylight saving time gap scenario, Google and Outlook export steps, versioned capture metadata, rendering buckets, privacy boundary, and corpus admission fields.
`capture-audit` reports hashes and structural readiness.
The report makes no claim about export anonymization.

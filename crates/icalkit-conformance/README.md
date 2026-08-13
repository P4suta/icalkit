# icalkit-conformance

`icalkit-conformance` is an unpublished test corpus and process-isolated subject for icalkit.
Its stable boundary is newline-delimited JSON on stdin/stdout, so another implementation can
drive or reproduce a case without depending on workspace internals.

The package also has an unpublished helper library target for its own white-box adversarial
tests. That target compiles icalkit's private module tree as shared source in an isolated crate
root; it is not a consumer API or a semver contract. Runtime subject operations use only the
public `icalkit` facade.

Protocol version `icalkit-conformance/1` accepts one object per line:

```json
{"protocol":"icalkit-conformance/1","id":"case-1","operation":"strict-parse","input_hex":"424547494e3a5643414c454e4441520d0a..."}
```

Supported operations are `strict-parse`, `normalize-rfc-repair-v1`, and
`normalize-common-clients-v1`. Every response repeats the protocol and request `id`, reports an
`outcome`, and uses stable string codes for errors, issues, and normalization changes. One bad
line yields a `protocol-error` response and does not terminate the process.

The versioned corpus manifest is `corpus/manifest.v1.jsonl`. Existing client-shaped cases are
marked `synthetic`; they are not evidence for a compatibility repair. A captured Google Calendar,
Microsoft 365, or Apple Calendar row must record its version and observation date and attest its
reduction and anonymization before it can justify `CommonClientsV1` behavior.

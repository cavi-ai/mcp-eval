# Troubleshooting

## No findings appear

Run `mcpeval index` after new capture data, then run `mcpeval promote` before `mcpeval findings`. Promotion requires evidence in two distinct sessions regardless of the selected threshold.

## A probe will not start

MCP Eval validates the manifest, case selection, mutation authorization, server target, and required tool declarations before running cases. Check the error and then confirm:

- the manifest has `"version": 1` and no unknown fields;
- the selected case or probe kind exists;
- a stdio command or `--url` is provided, but not both;
- every required tool is returned by the server's `tools/list`;
- every mutating case names a declared sandbox and the command includes `--allow-mutation`.

## An HTTP endpoint is rejected

Loopback HTTP is accepted by default. A remote endpoint must use HTTPS and requires `--allow-remote-http`. Remove credentials, query strings, and fragments from the URL. Responses are limited to 8 MiB and use five-second connect, read, and write timeouts.

## Check the store boundary

Run the hygiene scan:

```sh
mcpeval doctor --check-redaction
```

The command exits non-zero when a non-note JSONL field looks unredacted and prints the salt path as a must-not-share reminder. Annotation notes are exempt from automated redaction detectors because they contain deliberate user prose. When notes exist, `doctor` prints a non-failing review warning and can still exit zero, so a successful check does not prove those notes are safe. Manually review or remove every annotation note before sharing store records. Never include the sibling `.salt`, manifests, configuration, or `index.db`.

## Generation is refused

`generate` accepts only a current promoted finding with a valid tool and exactly empty shaped arguments. It also requires `--confirm-read-only`. By default it creates a new file; use `--force` to replace an existing output.

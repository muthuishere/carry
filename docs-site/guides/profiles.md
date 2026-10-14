# Profiles

Profiles work like kubectl contexts. They let the CLI on your laptop or in CI talk to several repositories without exporting variables each time.

## Config file

Default location `~/.config/carry/config.yaml` (override with `CARRY_CONFIG`):

```yaml
current-profile: prod
profiles:
  prod:
    repo: s3://my-bucket/carry
    endpoint: https://fsn1.your-objectstorage.com
    region: eu-central
    credentials: { env: PROD_S3 }      # reads PROD_S3_ACCESS_KEY_ID / PROD_S3_SECRET_ACCESS_KEY
    key: { file: ~/.config/carry/prod.key }
    readonly: false
  staging:
    repo: s3://staging-bucket/carry
    credentials: { file: ~/.config/carry/staging.credentials }
    key: { env: STAGING_CARRY_KEY }
  prod-ro:
    repo: s3://my-bucket/carry
    endpoint: https://fsn1.your-objectstorage.com
    credentials: { env: PROD_S3_RO }
    key: { file: ~/.config/carry/prod.key }
    readonly: true
```

| Field | Meaning |
|---|---|
| `repo` | `s3://bucket/prefix` |
| `endpoint` | S3 endpoint URL (omit for AWS) |
| `region` | S3 region |
| `credentials` | `{env: PREFIX}` reads `PREFIX_ACCESS_KEY_ID` and `PREFIX_SECRET_ACCESS_KEY`; `{file: path}` reads a file with `access_key_id=` / `secret_access_key=` lines (also accepts the `aws_` prefixed names) |
| `key` | Master key: `{env: VAR}` or `{file: path}`. Never inline |
| `readonly` | `true` refuses every write command with `readonly` (exit 2) |

Secrets are only ever *referenced* (`env` / `file`), never stored in the YAML.

## Commands

```sh
carry profile list          # {"current": "prod", "profiles": ["prod", "prod-ro", "staging"]}
carry profile use staging   # rewrites current-profile
carry profile current       # {"current": "staging"}
carry --profile prod-ro log web-1
CARRY_PROFILE=prod carry states
```

## Precedence

For every setting: **flag > env > launcher file > profile > default**.

- Profile selection: `--profile` > `CARRY_PROFILE` > `current-profile`.
- Credentials: `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` in the environment beat the profile.
- Master key: `CARRY_KEY` > `CARRY_KEY_FILE` > profile `key`.
- No command ever prompts.

## Read-only profiles

Give auditors or dashboards a `readonly: true` profile backed by read-only S3 credentials. Write commands (`init`, `put`, `rm`, `mv`, `rollback`, `env set/unset/import`, `tool publish/promote/yank/add/rm`) fail fast with `{"code": "readonly"}`. The S3 policy is the real enforcement; the profile flag just fails earlier with a clear message.

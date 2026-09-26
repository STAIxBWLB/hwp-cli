[한국어](README.ko.md) · [English](README.md)

# hwp MCP on AWS Bedrock AgentCore

The AgentCore tier (Tier B) of
[docs/design/22-remote-mcp-deployment.md](../../docs/design/22-remote-mcp-deployment.md): `hwp serve`
running as an MCP server on AgentCore Runtime. Every command below ran on 2026-09-26 against
v1.1.0 in us-east-1, when issue #318 was checked; the update command is the one exception, checked
only against the CLI's help.

## At a glance

- Four pieces: an arm64 image in ECR, an IAM execution role, an AgentCore Runtime with the MCP
  protocol, and inbound auth (IAM SigV4, or a JWT from Amazon Cognito).
- The image is [Dockerfile.agentcore](Dockerfile.agentcore), built unchanged. The container serves
  Streamable HTTP at `0.0.0.0:8000/mcp` and lists 22 tools.
- AgentCore exposes only `/mcp`, so the `/files` sideband of the Cloudflare tier does not exist.
  Documents travel inline through `hwp_put_file` and `hwp_get_file`: at most 512 KiB decoded per
  call, inside the 1 MiB request limit.
- Request framing is settled (#318): AgentCore forwards `/mcp` bodies length-framed on platform V1
  and V2, and de-chunks a client's HTTP/1.1 chunked body. `hwp serve` answers `411` to any
  `Transfer-Encoding`, and that never fires here.
- About 15 minutes end to end. A test run costs cents; a runtime with no session costs nothing.

## Prerequisites

- An AWS account whose service control policies allow `bedrock-agentcore`, private `ecr`,
  `iam` role creation and `logs` (and `cognito-idp` for JWT auth). An organization sandbox account
  may deny these in every region; the error then says `explicit deny in a service control policy`.
- An IAM principal, not the root user.
- A region where AgentCore Runtime runs. Platform V2 (faster cold starts, idle memory reclaimed)
  runs only in us-east-1, us-east-2, us-west-2, eu-west-1 and ap-northeast-1; other regions offer
  V1.
- A current AWS CLI v2 (one that has `aws login`; 2.37 was used) and Docker with buildx. On Apple
  Silicon with Homebrew:

  ```bash
  brew install awscli colima docker docker-buildx
  mkdir -p ~/.docker    # then add "cliPluginsExtraDirs": ["/opt/homebrew/lib/docker/cli-plugins"]
                        # to ~/.docker/config.json so docker finds buildx
  colima start --arch aarch64 --cpu 2 --memory 4 --disk 20
  ```

  On an x86_64 host, install binfmt first as the Dockerfile header shows.

Shell variables used below. Keep the braces: in zsh, `$ACCOUNT_ID:repository` reads `:r` as a
path modifier and silently breaks the ARN.

```bash
export AWS_PROFILE=<profile> AWS_REGION=us-east-1
ACCOUNT_ID=<12-digit account id>
REPO=hwp-agentcore
TAG=v1.1.0
IMAGE="${ACCOUNT_ID}.dkr.ecr.${AWS_REGION}.amazonaws.com/${REPO}:${TAG}"
```

## Sign in the CLI

`aws login` turns a console session into temporary CLI credentials; no access keys are needed.

```bash
aws login --profile <profile> --region us-east-1   # pick the IAM user's session in the browser
aws sts get-caller-identity --profile <profile>
```

Run `aws logout --profile <profile>` when done.

## Build and check the image locally

From the repository root. The Dockerfile downloads the release tarball and checks its sha256, so
the build needs no local files.

```bash
docker buildx build --platform linux/arm64 \
  -f deploy/aws/Dockerfile.agentcore -t ${REPO}:${TAG} --load deploy/aws

docker run -d --name hwpt --platform linux/arm64 -p 8000:8000 ${REPO}:${TAG}
H=(-H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream')
curl -s "${H[@]}" http://localhost:8000/mcp -d '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
  | python3 -c "import json,sys;print(len(json.load(sys.stdin)['result']['tools']))"   # 22
printf '{"jsonrpc":"2.0","id":3,"method":"tools/list"}' | curl -s -w ' %{http_code}\n' "${H[@]}" \
  -H 'Transfer-Encoding: chunked' --data-binary @- http://localhost:8000/mcp   # length required 411
time docker stop hwpt && docker rm hwpt   # about 2 s, not the 10 s kill timeout
```

## Push to ECR

```bash
aws ecr create-repository --repository-name ${REPO} --image-tag-mutability IMMUTABLE
aws ecr get-login-password | docker login --username AWS \
  --password-stdin ${ACCOUNT_ID}.dkr.ecr.${AWS_REGION}.amazonaws.com
docker tag ${REPO}:${TAG} ${IMAGE}
docker push ${IMAGE}
```

Tags are immutable; a new release goes up under a new tag. AgentCore accepts the manifest list
buildx pushes, attestation included.

## Execution role

The runtime assumes this role to pull the image and write logs, metrics and traces.

```bash
cat > trust.json <<EOF
{"Version":"2012-10-17","Statement":[{"Effect":"Allow",
  "Principal":{"Service":"bedrock-agentcore.amazonaws.com"},"Action":"sts:AssumeRole",
  "Condition":{"StringEquals":{"aws:SourceAccount":"${ACCOUNT_ID}"},
    "ArnLike":{"aws:SourceArn":"arn:aws:bedrock-agentcore:${AWS_REGION}:${ACCOUNT_ID}:*"}}}]}
EOF
cat > exec-policy.json <<EOF
{"Version":"2012-10-17","Statement":[
 {"Sid":"EcrPull","Effect":"Allow","Action":["ecr:BatchGetImage","ecr:GetDownloadUrlForLayer"],
  "Resource":"arn:aws:ecr:${AWS_REGION}:${ACCOUNT_ID}:repository/${REPO}"},
 {"Sid":"EcrToken","Effect":"Allow","Action":"ecr:GetAuthorizationToken","Resource":"*"},
 {"Sid":"Logs","Effect":"Allow","Action":["logs:CreateLogGroup","logs:CreateLogStream","logs:PutLogEvents",
  "logs:DescribeLogStreams","logs:DescribeLogGroups"],
  "Resource":["arn:aws:logs:${AWS_REGION}:${ACCOUNT_ID}:log-group:/aws/bedrock-agentcore/runtimes/*",
   "arn:aws:logs:${AWS_REGION}:${ACCOUNT_ID}:log-group:*"]},
 {"Sid":"Metrics","Effect":"Allow","Action":"cloudwatch:PutMetricData","Resource":"*",
  "Condition":{"StringEquals":{"cloudwatch:namespace":"bedrock-agentcore"}}},
 {"Sid":"Xray","Effect":"Allow","Action":["xray:PutTraceSegments","xray:PutTelemetryRecords",
  "xray:GetSamplingRules","xray:GetSamplingTargets"],"Resource":"*"},
 {"Sid":"WorkloadToken","Effect":"Allow","Action":["bedrock-agentcore:GetWorkloadAccessToken",
  "bedrock-agentcore:GetWorkloadAccessTokenForJWT","bedrock-agentcore:GetWorkloadAccessTokenForUserId"],
  "Resource":["arn:aws:bedrock-agentcore:${AWS_REGION}:${ACCOUNT_ID}:workload-identity-directory/default",
   "arn:aws:bedrock-agentcore:${AWS_REGION}:${ACCOUNT_ID}:workload-identity-directory/default/workload-identity/*"]}
]}
EOF
aws iam create-role --role-name hwp-mcp-exec --assume-role-policy-document file://trust.json
aws iam put-role-policy --role-name hwp-mcp-exec --policy-name hwp-mcp-exec \
  --policy-document file://exec-policy.json
ROLE_ARN="arn:aws:iam::${ACCOUNT_ID}:role/hwp-mcp-exec"
```

## Inbound auth

- **IAM (SigV4)** is the default when no authorizer is given. It suits internal checks and
  automation that call through the AWS CLI or an SDK.
- **JWT** is what MCP clients and an Amazon Quick connector use, and what Tier B runs in
  production (a Cognito user pool, later with Google as a federated IdP). A test pool:

  ```bash
  umask 077
  POOL=$(aws cognito-idp create-user-pool --pool-name hwp-mcp \
    --policies '{"PasswordPolicy":{"MinimumLength":12}}' --query 'UserPool.Id' --output text)
  CLIENT=$(aws cognito-idp create-user-pool-client --user-pool-id $POOL --client-name hwp-mcp \
    --no-generate-secret --explicit-auth-flows ALLOW_USER_PASSWORD_AUTH ALLOW_REFRESH_TOKEN_AUTH \
    --query 'UserPoolClient.ClientId' --output text)
  PW=$(python3 -c "import secrets,string;a=string.ascii_letters+string.digits;print(''.join(secrets.choice(a) for _ in range(20))+'Aa1!')")
  aws cognito-idp admin-create-user --user-pool-id $POOL --username tester --message-action SUPPRESS
  aws cognito-idp admin-set-user-password --user-pool-id $POOL --username tester --password "$PW" --permanent
  aws cognito-idp initiate-auth --client-id $CLIENT --auth-flow USER_PASSWORD_AUTH \
    --auth-parameters "USERNAME=tester,PASSWORD=$PW" \
    --query 'AuthenticationResult.AccessToken' --output text > token   # valid for 1 hour
  unset PW
  JWT="{\"customJWTAuthorizer\":{\"discoveryUrl\":\"https://cognito-idp.${AWS_REGION}.amazonaws.com/${POOL}/.well-known/openid-configuration\",\"allowedClients\":[\"${CLIENT}\"]}}"
  ```

## Create the runtime

```bash
# IAM auth, default platform (V1)
aws bedrock-agentcore-control create-agent-runtime --agent-runtime-name hwp_mcp_iam \
  --agent-runtime-artifact "{\"containerConfiguration\":{\"containerUri\":\"${IMAGE}\"}}" \
  --role-arn ${ROLE_ARN} --network-configuration networkMode=PUBLIC \
  --protocol-configuration serverProtocol=MCP

# JWT auth, platform V2
aws bedrock-agentcore-control create-agent-runtime --agent-runtime-name hwp_mcp_jwt \
  --agent-runtime-artifact "{\"containerConfiguration\":{\"containerUri\":\"${IMAGE}\"}}" \
  --role-arn ${ROLE_ARN} --network-configuration networkMode=PUBLIC \
  --protocol-configuration serverProtocol=MCP --authorizer-configuration "$JWT" \
  --platform-version V2

aws bedrock-agentcore-control list-agent-runtimes \
  --query 'agentRuntimes[].[agentRuntimeName,status,agentRuntimeArn]' --output table   # READY in about a minute
```

Runtime names start with a letter and use only letters, digits and underscores.

## Invoke

IAM runtime, through `InvokeAgentRuntime`:

```bash
ARN=$(aws bedrock-agentcore-control list-agent-runtimes \
  --query 'agentRuntimes[?agentRuntimeName==`hwp_mcp_iam`].agentRuntimeArn' --output text)
aws bedrock-agentcore invoke-agent-runtime --agent-runtime-arn "$ARN" \
  --content-type application/json --accept "application/json, text/event-stream" \
  --cli-binary-format raw-in-base64-out \
  --payload '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' out.json
python3 -c "import json;print(len(json.load(open('out.json'))['result']['tools']))"   # 22
```

JWT runtime, over HTTPS with the bearer token. This URL is also what an MCP client (MCP
Inspector, an Amazon Quick connector) is given.

```bash
ARN=$(aws bedrock-agentcore-control list-agent-runtimes \
  --query 'agentRuntimes[?agentRuntimeName==`hwp_mcp_jwt`].agentRuntimeArn' --output text)
ENC=$(python3 -c "import urllib.parse,sys;print(urllib.parse.quote(sys.argv[1],safe=''))" "$ARN")
URL="https://bedrock-agentcore.${AWS_REGION}.amazonaws.com/runtimes/${ENC}/invocations?qualifier=DEFAULT"
printf 'Authorization: Bearer %s\n' "$(cat token)" > auth.hdr
C=(-s -H @auth.hdr -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' -A 'hwp-mcp-client/1.0')
curl "${C[@]}" -D h.txt --data-binary \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}' "$URL"
SESS=$(grep -i '^mcp-session-id:' h.txt | awk '{print $2}' | tr -d '\r')
curl "${C[@]}" -H "Mcp-Session-Id: $SESS" --data-binary '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' "$URL"
rm -f auth.hdr
```

- Send the returned `Mcp-Session-Id` on every later request so they reach the same microVM and
  skip another cold start.
- `Accept` must list both `application/json` and `text/event-stream`, or the platform answers
  `406`.
- A missing or bad token gets `401` with `WWW-Authenticate` from a JWT runtime, and `403` from an
  IAM runtime.

## Update to a new release

Bump `HWP_VERSION` and `HWP_SHA256` in [Dockerfile.agentcore](Dockerfile.agentcore) together (the
sha256 is published beside the tarball as `hwp-<version>-aarch64-unknown-linux-gnu.sha256`), build
and push under the new tag, then point the runtime at it. `update-agent-runtime` takes the artifact
and role again; pass the auth and protocol settings as at creation.

```bash
aws bedrock-agentcore-control update-agent-runtime --agent-runtime-id <id> \
  --agent-runtime-artifact "{\"containerConfiguration\":{\"containerUri\":\"${IMAGE}\"}}" \
  --role-arn ${ROLE_ARN} --network-configuration networkMode=PUBLIC \
  --protocol-configuration serverProtocol=MCP --authorizer-configuration "$JWT"
```

## Tear down

Runtimes first, then the role.

```bash
for id in $(aws bedrock-agentcore-control list-agent-runtimes \
    --query 'agentRuntimes[?starts_with(agentRuntimeName, `hwp_mcp`)].agentRuntimeId' --output text); do
  aws bedrock-agentcore-control delete-agent-runtime --agent-runtime-id $id
done
aws logs describe-log-groups --log-group-name-prefix /aws/bedrock-agentcore/runtimes/hwp_mcp \
  --query 'logGroups[].logGroupName' --output text | tr '\t' '\n' \
  | while read g; do [ -n "$g" ] && aws logs delete-log-group --log-group-name "$g"; done
aws iam delete-role-policy --role-name hwp-mcp-exec --policy-name hwp-mcp-exec
aws iam delete-role --role-name hwp-mcp-exec
aws ecr delete-repository --repository-name ${REPO} --force
aws cognito-idp delete-user-pool --user-pool-id $POOL
rm -f token
```

Check `aws bedrock-agentcore-control list-workload-identities` for identities a JWT runtime may
have created. The Resource Groups tagging index lags deletions, so confirm with the per-service
describe calls.

## Cost

Checked on 2026-09-26 (us-east-1, AWS pricing pages).

| Item | Price |
|---|---|
| Runtime V1 | $0.0895 per vCPU-hour, $0.00945 per GB-hour, billed per second |
| Runtime V2 | $0.1276 per vCPU-hour, $0.0169 per GB-hour |
| ECR storage | $0.10 per GB-month (the image is about 53 MB compressed) |
| CloudWatch Logs | $0.50 per GB ingested, 5 GB per month free |
| Cognito | free up to 10,000 monthly active users |

CPU is not billed during I/O wait, and nothing is billed without a session. The #318 check cost a
few cents. The large Tier B cost is Amazon Quick, not AgentCore: $20 or $40 per user per month plus
a $250 per-account infrastructure fee. Put an AWS Budgets alarm in place before a first deploy.

## Verified

The #318 check, 2026-09-26, three runtimes on the v1.1.0 image:

| Runtime | Request | Result |
|---|---|---|
| IAM, V1 | `invoke-agent-runtime`: `initialize`, then `tools/list` | 200, 22 tools |
| JWT, V1 and V2 | HTTP/1.1 and HTTP/2, `Content-Length` | 200, 22 tools |
| JWT, V1 and V2 | HTTP/1.1, `Transfer-Encoding: chunked` | 200, 22 tools (de-chunked by the platform) |

## Troubleshooting

- **`explicit deny in a service control policy`**: an organization policy blocks the service in
  that account. Lift it from the management account or use another account.
- **`MalformedPolicyDocument` (failed legacy parsing)**: zsh rewrote `$ACCOUNT_ID:r...` as a path
  modifier. Use `${ACCOUNT_ID}`.
- **A JSON-RPC parse error for a chunked request sent with curl**: over HTTP/2, which has no chunked
  coding, a forced `Transfer-Encoding: chunked` header makes curl send the chunk framing as body
  bytes. Test chunked bodies with `--http1.1`.
- **`Session operation in progress, please retry`** (JSON-RPC `-32005`, HTTP 200): a request
  overlapped a session being created or torn down. Retry after a short backoff.
- **Requests from the MCP Python SDK rejected**: AWS WAF rules may block requests without a
  `User-Agent`; set one on the client.

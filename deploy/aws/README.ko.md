[한국어](README.ko.md) · [English](README.md)

# AWS Bedrock AgentCore의 hwp MCP

[docs/design/22-remote-mcp-deployment.ko.md](../../docs/design/22-remote-mcp-deployment.ko.md)의
AgentCore tier(Tier B)다. `hwp serve`를 AgentCore Runtime에서 MCP 서버로 돌린다. 아래 명령은
2026-09-26 이슈 #318을 확인할 때 us-east-1에서 v1.1.0으로 실제로 실행했다. 예외는 두 가지다. 버전
교체 명령은 `UpdateAgentRuntime` API 문서로만 확인했고, 실행 역할의 로그 권한은 검증 뒤 AWS 실행
역할 예시에 맞춰 좁혔다(검증 때는 `log-group:*`).

## 요약

- 구성은 네 가지다: ECR의 arm64 이미지, IAM 실행 역할, MCP 프로토콜의 AgentCore Runtime, 인바운드
  인증(IAM SigV4, 또는 Amazon Cognito의 JWT).
- 이미지는 [Dockerfile.agentcore](Dockerfile.agentcore)를 그대로 빌드한다. 컨테이너는
  `0.0.0.0:8000/mcp`에서 Streamable HTTP로 응답하고 도구 22개를 낸다.
- AgentCore는 `/mcp`만 노출하므로 Cloudflare tier의 `/files` 업로드는 없다. 문서는 `hwp_put_file`과
  `hwp_get_file`로 인라인 전송하며, 한 번에 디코딩 기준 512 KiB, 요청 한 건은 1 MiB까지다.
- request framing은 확정됐다(#318). AgentCore는 플랫폼 V1과 V2 모두 `/mcp` 본문을 길이 지정으로
  넘기고, 클라이언트의 HTTP/1.1 chunked 본문은 풀어서 넘긴다. `hwp serve`는 `Transfer-Encoding`에
  `411`로 답하지만 여기서는 그럴 일이 없다.
- 처음부터 끝까지 약 15분. 검증 1회는 수 센트이고, 세션이 없는 런타임은 과금되지 않는다.

## 준비

- 서비스 제어 정책(SCP)이 `bedrock-agentcore`, 비공개 `ecr`, `iam` 역할 생성, `logs`(JWT 인증이면
  `cognito-idp`도)를 허용하는 AWS 계정. 조직 샌드박스 계정은 이를 전 리전에서 막을 수 있고, 그때
  오류에 `explicit deny in a service control policy`가 나온다.
- root 사용자가 아닌 IAM 주체.
- AgentCore Runtime이 도는 리전. 플랫폼 V2(빠른 콜드 스타트, 유휴 메모리 회수)는 us-east-1,
  us-east-2, us-west-2, eu-west-1, ap-northeast-1에서만 되고 다른 리전은 V1이다.
- 최신 AWS CLI v2(`aws login`이 있는 버전, 2.37 사용)와 buildx가 있는 Docker. Apple Silicon과
  Homebrew 기준:

  ```bash
  brew install awscli colima docker docker-buildx
  mkdir -p ~/.docker    # docker가 buildx를 찾도록 ~/.docker/config.json에
                        # "cliPluginsExtraDirs": ["/opt/homebrew/lib/docker/cli-plugins"]를 추가
  colima start --arch aarch64 --cpu 2 --memory 4 --disk 20
  ```

  x86_64 호스트는 Dockerfile 머리말대로 binfmt를 먼저 설치한다.

아래에서 쓰는 셸 변수. 중괄호를 지킨다. zsh는 `$ACCOUNT_ID:repository`의 `:r`을 경로 수식자로
읽어 ARN을 조용히 깨뜨린다.

```bash
export AWS_PROFILE=<profile> AWS_REGION=us-east-1
ACCOUNT_ID=<12자리 계정 ID>
REPO=hwp-agentcore
TAG=v1.1.0
IMAGE="${ACCOUNT_ID}.dkr.ecr.${AWS_REGION}.amazonaws.com/${REPO}:${TAG}"
```

## CLI 로그인

`aws login`은 콘솔 세션을 CLI 임시 자격 증명으로 바꾼다. 액세스 키가 필요 없다.

```bash
aws login --profile <profile> --region us-east-1   # 브라우저에서 IAM 사용자 세션을 고른다
aws sts get-caller-identity --profile <profile>
```

끝나면 `aws logout --profile <profile>`로 지운다.

## 이미지 빌드와 로컬 확인

저장소 루트에서 실행한다. Dockerfile이 릴리스 tarball을 받아 sha256을 확인하므로 로컬 파일이
필요 없다.

```bash
docker buildx build --platform linux/arm64 \
  -f deploy/aws/Dockerfile.agentcore -t ${REPO}:${TAG} --load deploy/aws

docker run -d --name hwpt --platform linux/arm64 -p 8000:8000 ${REPO}:${TAG}
H=(-H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream')
curl -s "${H[@]}" http://localhost:8000/mcp -d '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
  | python3 -c "import json,sys;print(len(json.load(sys.stdin)['result']['tools']))"   # 22
printf '{"jsonrpc":"2.0","id":3,"method":"tools/list"}' | curl -s -w ' %{http_code}\n' "${H[@]}" \
  -H 'Transfer-Encoding: chunked' --data-binary @- http://localhost:8000/mcp   # length required 411
time docker stop hwpt && docker rm hwpt   # 10초 강제 종료가 아니라 약 2초
```

## ECR 푸시

```bash
aws ecr create-repository --repository-name ${REPO} --image-tag-mutability IMMUTABLE
aws ecr get-login-password | docker login --username AWS \
  --password-stdin ${ACCOUNT_ID}.dkr.ecr.${AWS_REGION}.amazonaws.com
docker tag ${REPO}:${TAG} ${IMAGE}
docker push ${IMAGE}
```

태그는 불변이다. 새 릴리스는 새 태그로 올린다. buildx가 올리는 매니페스트 목록(attestation
포함)도 AgentCore가 받는다.

## 실행 역할

런타임은 이 역할로 이미지를 받고 로그·지표·추적을 쓴다.

```bash
WORK=$(mktemp -d) && cd "$WORK"   # 계정 값이 든 파일과 토큰을 체크아웃 밖에 둔다
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
 {"Sid":"LogGroups","Effect":"Allow","Action":["logs:CreateLogGroup","logs:DescribeLogStreams"],
  "Resource":"arn:aws:logs:${AWS_REGION}:${ACCOUNT_ID}:log-group:/aws/bedrock-agentcore/runtimes/*"},
 {"Sid":"LogDescribe","Effect":"Allow","Action":"logs:DescribeLogGroups",
  "Resource":"arn:aws:logs:${AWS_REGION}:${ACCOUNT_ID}:log-group:*"},
 {"Sid":"LogEvents","Effect":"Allow","Action":["logs:CreateLogStream","logs:PutLogEvents"],
  "Resource":"arn:aws:logs:${AWS_REGION}:${ACCOUNT_ID}:log-group:/aws/bedrock-agentcore/runtimes/*:log-stream:*"},
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

- 신뢰 정책: `bedrock-agentcore.amazonaws.com`만, 이 계정(`aws:SourceAccount`)의 런타임
  (`aws:SourceArn`)에서만 역할을 맡는다.
- 권한: 해당 ECR 저장소 pull과 인증 토큰, `/aws/bedrock-agentcore/runtimes/*` 아래 로그 그룹·스트림
  생성과 기록(로그 그룹 목록 조회만 `log-group:*`), `bedrock-agentcore` 네임스페이스 지표, X-Ray,
  기본 워크로드 ID 디렉터리의 워크로드 토큰.

## 인바운드 인증

- **IAM(SigV4)**: 인증 설정을 생략하면 기본값이다. AWS CLI나 SDK로 부르는 내부 검증과 자동화에
  맞다.
- **JWT**: MCP 클라이언트와 Amazon Quick 커넥터가 쓰는 방식이고 Tier B 본 가동도 이쪽이다(Cognito
  사용자 풀, 나중에 Google을 연합 IdP로). 시험용 풀은 다음과 같다. 비밀번호는 셸 변수로만 두고
  파일에 쓰지 않지만, 이를 받는 두 호출이 도는 동안에는 프로세스 목록에 보인다. 토큰은 권한 600
  파일에 두며 1시간 뒤 만료된다.

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
    --query 'AuthenticationResult.AccessToken' --output text > token   # 1시간 유효
  unset PW
  JWT="{\"customJWTAuthorizer\":{\"discoveryUrl\":\"https://cognito-idp.${AWS_REGION}.amazonaws.com/${POOL}/.well-known/openid-configuration\",\"allowedClients\":[\"${CLIENT}\"]}}"
  ```

## 런타임 생성

```bash
# IAM 인증, 기본 플랫폼(V1)
aws bedrock-agentcore-control create-agent-runtime --agent-runtime-name hwp_mcp_iam \
  --agent-runtime-artifact "{\"containerConfiguration\":{\"containerUri\":\"${IMAGE}\"}}" \
  --role-arn ${ROLE_ARN} --network-configuration networkMode=PUBLIC \
  --protocol-configuration serverProtocol=MCP

# JWT 인증, 플랫폼 V2
aws bedrock-agentcore-control create-agent-runtime --agent-runtime-name hwp_mcp_jwt \
  --agent-runtime-artifact "{\"containerConfiguration\":{\"containerUri\":\"${IMAGE}\"}}" \
  --role-arn ${ROLE_ARN} --network-configuration networkMode=PUBLIC \
  --protocol-configuration serverProtocol=MCP --authorizer-configuration "$JWT" \
  --platform-version V2

aws bedrock-agentcore-control list-agent-runtimes \
  --query 'agentRuntimes[].[agentRuntimeName,status,agentRuntimeArn]' --output table   # 1분 안에 READY
```

런타임 이름은 영문자로 시작하고 영문자·숫자·밑줄만 쓴다.

## 호출

IAM 런타임은 `InvokeAgentRuntime`으로 부른다.

```bash
ARN=$(aws bedrock-agentcore-control list-agent-runtimes \
  --query 'agentRuntimes[?agentRuntimeName==`hwp_mcp_iam`].agentRuntimeArn' --output text)
aws bedrock-agentcore invoke-agent-runtime --agent-runtime-arn "$ARN" \
  --content-type application/json --accept "application/json, text/event-stream" \
  --cli-binary-format raw-in-base64-out \
  --payload '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' out.json
python3 -c "import json;print(len(json.load(open('out.json'))['result']['tools']))"   # 22
```

JWT 런타임은 HTTPS에 Bearer 토큰으로 부른다. MCP 클라이언트(MCP Inspector, Amazon Quick 커넥터)에
넣는 주소도 이것이다.

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

- 응답의 `Mcp-Session-Id`를 이후 요청마다 실어야 같은 microVM으로 가서 콜드 스타트가 반복되지 않는다.
- `Accept`에 `application/json`과 `text/event-stream`을 모두 넣는다. 빠지면 `406`이다.
- 토큰이 없거나 틀리면 JWT 런타임은 `WWW-Authenticate`와 함께 `401`, IAM 런타임은 `403`이다.

## 새 릴리스로 교체

[Dockerfile.agentcore](Dockerfile.agentcore)의 `HWP_VERSION`과 `HWP_SHA256`을 함께 올리고(sha256은
tarball 옆 `hwp-<version>-aarch64-unknown-linux-gnu.sha256`), 새 태그로 빌드·푸시한 뒤 런타임이 새
이미지를 가리키게 한다. `update-agent-runtime`에는 생성 때 준 설정을 모두 다시 넘긴다.
아티팩트·역할·네트워크·프로토콜에 더해, JWT 런타임은 인증 설정을, V2 런타임은 플랫폼 버전을 넘긴다.

```bash
ID=$(aws bedrock-agentcore-control list-agent-runtimes \
  --query 'agentRuntimes[?agentRuntimeName==`hwp_mcp_jwt`].agentRuntimeId' --output text)
aws bedrock-agentcore-control update-agent-runtime --agent-runtime-id "$ID" \
  --agent-runtime-artifact "{\"containerConfiguration\":{\"containerUri\":\"${IMAGE}\"}}" \
  --role-arn ${ROLE_ARN} --network-configuration networkMode=PUBLIC \
  --protocol-configuration serverProtocol=MCP --authorizer-configuration "$JWT" \
  --platform-version V2
# IAM 런타임(hwp_mcp_iam)은 --authorizer-configuration과 --platform-version 없이 같은 명령.
```

## 삭제

런타임을 먼저, 역할은 나중에 지운다.

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
cd && rm -rf "$WORK"   # trust.json, exec-policy.json, token, out.json, h.txt
```

JWT 런타임이 만든 워크로드 ID는 `aws bedrock-agentcore-control list-workload-identities`로 확인한다.
Resource Groups 태그 색인은 삭제를 늦게 반영하므로 서비스별 describe로 재확인한다.

## 비용

2026-09-26 us-east-1 AWS 가격 페이지 기준.

| 항목 | 단가 |
|---|---|
| Runtime V1 | vCPU 시간당 $0.0895, GB 시간당 $0.00945, 초 단위 |
| Runtime V2 | vCPU 시간당 $0.1276, GB 시간당 $0.0169 |
| ECR 저장 | GB-월 $0.10 (이미지 압축 약 53 MB) |
| CloudWatch Logs | 수집 GB당 $0.50, 월 5 GB 무료 |
| Cognito | 월 10,000 MAU까지 무료 |

I/O 대기 중 CPU는 과금되지 않고 세션이 없으면 과금되지 않는다. #318 검증은 수 센트였다. Tier B의
큰 비용은 AgentCore가 아니라 Amazon Quick이다(사용자당 월 $20 또는 $40, 계정당 월 $250 인프라
비용). 첫 배포 전에 AWS Budgets 경보를 걸어 둔다.

## 검증 기록

2026-09-26 #318 확인, v1.1.0 이미지로 런타임 3개:

| 런타임 | 요청 | 결과 |
|---|---|---|
| IAM, V1 | `invoke-agent-runtime`으로 `initialize`, `tools/list` | 200, 도구 22개 |
| JWT, V1·V2 | HTTP/1.1·HTTP/2, `Content-Length` | 200, 도구 22개 |
| JWT, V1·V2 | HTTP/1.1, `Transfer-Encoding: chunked` | 200, 도구 22개(플랫폼이 chunked를 풀어 전달) |

## 문제 해결

- **`explicit deny in a service control policy`**: 조직 정책이 그 계정에서 서비스를 막는다. 관리
  계정에서 풀거나 다른 계정을 쓴다.
- **`MalformedPolicyDocument`(failed legacy parsing)**: zsh가 `$ACCOUNT_ID:r...`를 경로 수식자로
  바꿨다. `${ACCOUNT_ID}`로 쓴다.
- **curl로 보낸 chunked 요청의 JSON-RPC 파싱 오류**: chunked 전송이 없는 HTTP/2에서
  `Transfer-Encoding: chunked`를 강제하면 curl이 청크 표식을 본문 바이트로 보낸다. chunked 시험은
  `--http1.1`로 한다.
- **`Session operation in progress, please retry`**(JSON-RPC `-32005`, HTTP 200): 세션 생성·정리 중에
  요청이 겹쳤다. 잠시 뒤 재시도한다.
- **MCP Python SDK 요청이 거부됨**: AWS WAF 규칙이 `User-Agent` 없는 요청을 막을 수 있다. 클라이언트에
  설정한다.

[한국어](README.md) · [English](README.en.md)

# 독립 오라클 빌드 감사

`primary-artifacts.lock.json`은 독립적으로 내려받아 해시를 검증한, 정확한 공식 1차 산출물만
기록한다. 이미지 lock이 아니며, 필수 인증 오라클을 사용 가능하게 만들지도 않는다.

Dockerfile은 아직 공개하지 않았다. LibreOffice DEB 아카이브는 아카이브 해시로 포착되지 않는
운영체제 런타임 폐포에 의존하고, 베이스 이미지 digest도 아직 고르지 않았다. 재현 가능한 빌드에는
실제로 빌드된 이미지 ID·repository digest와 오프라인 러너 증명도 필요하다. 이 값들을 지어내면
오라클 결과를 검증할 수 없게 된다.

H2Orestart의 OXT에는 `COPYING` 파일이 빠져 있다. 앞으로 OXT를 재배포하는 이미지는 GPL 고지와
대응 소스 제공 수단을 갖춰야 한다. 운영자가 마운트해 제공하는 OXT는 다른 배포 모델이지만, 그래도
인증 정책이 해시로 증명해야 한다.

lock의 미충족 요건이 모두 해소되기 전까지 `oracle.mode=required`는 네이티브 렌더러로 조용히
폴백하지 않고 partial/not-run을 올바르게 반환한다. 구조 코퍼스는 `oracle.mode=disabled`를 쓰며
독립 오피스나 한컴 동등성을 주장하지 않는다.

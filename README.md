# HamR API Gateway

统一 API 入口，负责路由转发、JWT 鉴权和限流。

## 路由规则

| 路径前缀 | 转发目标 | 说明 |
|---------|---------|------|
| `/api/v1/account/*` | hamr-account-api:8080 | 账号中心 |
| `/api/v1/app/*` | hamr-app-api:8081 | 管家应用 |
| `/api/v1/jiabu/*` | hamr-jiabu-api:8082 | JiaBu 决策 |
| `/health` | - | 健康检查（公开） |

## 中间件

- **JWT 鉴权**：所有 `/api/v1/*` 路由需携带 `Authorization: Bearer <token>`
- **限流**：每 IP 每分钟默认 60 次请求（可配置）

## 环境变量

```bash
cp .env.example .env
# 编辑 .env 填写实际配置
```

## 本地运行

```bash
cargo run
```

## Docker 构建

```bash
docker build -t hamr-api-gateway .
docker run -p 8090:8090 --env-file .env hamr-api-gateway
```

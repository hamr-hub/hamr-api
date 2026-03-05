# HamR API 服务 (api.hamr.top)

> HamR 统一 API 网关 - 安全、高效、可观测

[![Status](https://img.shields.io/badge/status-开发中-yellow)](https://github.com/hamr-hub/hamr-api)
[![License](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![Backend](https://img.shields.io/badge/backend-Rust+Axum-orange)](https://github.com/tokio-rs/axum)

## 📋 项目概述

**项目编号**: PROJ-008  
**域名**: api.hamr.top  
**优先级**: ⭐⭐⭐ 高  
**状态**: 待开发

HamR API 服务是平台的统一入口，负责流量管理、安全防护、监控日志、开发者支持。

## 🎯 核心职责

### 1. 统一入口管理
- 所有外部 API 请求经网关分发
- 路由到后端微服务 (account/app/jiabu)
- 负载均衡与故障转移

### 2. 安全防护
- **API Key + OAuth2 鉴权**
- **滑动窗口限流**: 1000 req/min/IP
- **IP 黑白名单**: 防止恶意请求
- **HMAC 签名验证**: 请求完整性校验

### 3. 流量治理
- 智能路由
- 熔断降级
- 限流削峰
- 超时控制

### 4. 可观测性
- **请求日志**: 全量记录
- **性能监控**: P50/P90/P99 延迟
- **分布式追踪**: OpenTelemetry
- **告警通知**: 异常自动告警

### 5. 开发者支持
- API Key 管理（创建/撤销/续期）
- OpenAPI 文档自动生成
- SDK 代码生成
- Webhook 回调支持

## 🏗️ 系统架构

```
      ┌──────────┐
      │  Client  │
      └────┬─────┘
           │ HTTPS
      ┌────▼─────┐
      │   API    │  Rust + Axum
      │ Gateway  │  api.hamr.top
      └────┬─────┘
           │
    ┌──────┼──────┬────────┐
    │      │      │        │
┌───▼───┐ ┌▼───┐ ┌▼────┐ ┌▼─────┐
│Account│ │App │ │JiaBu│ │Status│
│Service│ │Svc │ │ Svc │ │ Svc  │
└───────┘ └────┘ └─────┘ └──────┘
```

## 🛠️ 技术栈

| 技术 | 用途 | 备注 |
|-----|------|------|
| **Rust** | 编程语言 | 高性能 |
| **Axum** | Web 框架 | 异步网关 |
| **Redis** | 缓存/限流 | 滑动窗口 |
| **Prometheus** | 监控指标 | 时序数据 |
| **OpenTelemetry** | 分布式追踪 | Jaeger |
| **PostgreSQL** | 配置存储 | API Key |

## 🚀 快速开始

```bash
# 配置环境变量
cp .env.example .env

# 开发模式
cargo run

# 生产构建
cargo build --release
```

## 📦 项目结构

```
hamr-api/
├── src/
│   ├── main.rs              # 入口文件
│   ├── gateway/             # 网关核心
│   │   ├── router.rs        # 路由管理
│   │   ├── auth.rs          # 鉴权中间件
│   │   ├── rate_limit.rs    # 限流中间件
│   │   └── proxy.rs         # 代理转发
│   ├── services/            # 后端服务配置
│   ├── middleware/          # 中间件
│   └── utils/               # 工具函数
├── Cargo.toml
└── .env.example
```

## 🔌 路由表

```
/api/v1/auth/*         → account-service
/api/v1/persons/*      → app-service
/api/v1/tasks/*        → app-service
/api/v1/items/*        → app-service
/api/v1/insights/*     → jiabu-service
/api/v1/health         → gateway (自身健康检查)
```

## 🔐 安全特性

### API Key 认证
```http
GET /api/v1/persons
Authorization: Bearer your-api-key
```

### OAuth 2.0
```http
GET /api/v1/persons
Authorization: Bearer eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9...
```

### 限流策略
- 公共 API: 100 req/min/IP
- 认证 API: 1000 req/min/user
- 内部 API: 无限制

## 📊 里程碑

- [ ] **2026-03-20**: 需求确认
- [ ] **2026-04-15**: 网关开发
- [ ] **2026-04-30**: 鉴权限流
- [ ] **2026-05-10**: 监控日志
- [ ] **2026-05-20**: 测试上线

## 🔗 相关服务

- [账号中心](https://account.hamr.store)
- [HamR 管家](https://app.hamr.store)
- [JiaBu 决策](https://jiabu.hamr.store)

## 📄 许可证

MIT License

---

**最后更新**: 2026-03-05  
**部署环境**: https://api.hamr.top (即将上线)

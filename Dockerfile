FROM node:22-alpine AS frontend-builder

WORKDIR /app/admin-ui
COPY admin-ui/package.json ./
RUN npm install -g pnpm && pnpm install
COPY admin-ui ./
RUN pnpm build

FROM rust:1.92-alpine AS builder

RUN apk add --no-cache musl-dev openssl-dev openssl-libs-static

WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
COPY src ./src
COPY --from=frontend-builder /app/admin-ui/dist /app/admin-ui/dist

RUN cargo build --release

FROM alpine:3.21

RUN apk add --no-cache ca-certificates

WORKDIR /app
COPY --from=builder /app/target/release/kiro-rs /app/kiro-rs

# 创建配置目录和文件
RUN mkdir -p config && \
    # 生成config.json - 注意使用单引号避免变量扩展
    cat > config/config.json << 'EOF'
{
  "host": "0.0.0.0",
  "port": 8990,
  "apiKey": "sk-kiro-rs-default-api-key-change-me",
  "region": "us-east-1",
  "adminApiKey": "sk-admin-default-admin-key-change-me"
}
EOF

# 生成credentials.json
RUN echo '[]' > config/credentials.json


VOLUME ["/app/config"]

EXPOSE 8990

CMD ["./kiro-rs", "-c", "/app/config/config.json", "--credentials", "/app/config/credentials.json"]

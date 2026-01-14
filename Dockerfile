FROM node:22-alpine AS frontend-builder

WORKDIR /app/admin-ui
COPY admin-ui/package.json ./
RUN npm install -g pnpm && pnpm install
COPY admin-ui ./
RUN pnpm build

FROM rust:1.92-alpine AS builder

ARG FEATURES=""

RUN apk add --no-cache musl-dev openssl-dev openssl-libs-static

WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
COPY src ./src
COPY --from=frontend-builder /app/admin-ui/dist /app/admin-ui/dist

RUN cargo build --release ${FEATURES}

FROM alpine:3.21

RUN apk add --no-cache ca-certificates bash vim curl

WORKDIR /app
COPY --from=builder /app/target/release/kiro-rs /app/kiro-rs

# 创建配置目录和文件
RUN mkdir -p config && \
    # 生成config.json
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

RUN chmod -R 777 ./config

# 创建启动脚本
RUN cat > /start.sh << 'EOF'
#!/bin/sh
echo "=== Kiro.rs 启动脚本 ==="
echo "1. 启动应用: ./kiro-rs -c /app/config/config.json --credentials /app/config/credentials.json"
echo "2. 进入交互模式: /bin/bash"
echo "3. 检查配置文件: cat /app/config/config.json"
echo "================================="
/bin/bash
EOF

RUN chmod +x /start.sh

VOLUME ["/app/config"]

EXPOSE 8990

# 使用shell作为默认入口，这样容器不会自动退出
CMD tail -f /dev/null

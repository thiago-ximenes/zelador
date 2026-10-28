# Duas imagens em uma: o laco e um binario de 623 KB, mas ele invoca o `claude`, que e
# Node -- entao o runtime precisa do Node de qualquer jeito. O que o Rust economiza aqui
# e o processo de plantao (632 KiB em vez de ~13 MB), nao o tamanho da imagem (785 MB).
# Vender o contrario seria mentira.
FROM rust:1.90-alpine AS build
RUN apk add --no-cache build-base
WORKDIR /src
COPY Cargo.toml ./
COPY src ./src
RUN cargo build --release

FROM node:24-alpine
# docker-cli: o zelador conversa com o Docker do host pelo socket montado.
# git/openssh: ele commita e abre PR. bash: o que muitos scripts de projeto esperam.
RUN apk add --no-cache docker-cli docker-cli-compose git openssh-client bash ca-certificates curl \
 && npm i -g @anthropic-ai/claude-code@latest \
 && npm cache clean --force
COPY --from=build /src/target/release/zelador /usr/local/bin/zelador

# Nao-root por exigencia do proprio claude: ele recusa --dangerously-skip-permissions
# rodando como root ("cannot be used with root/sudo privileges"). O uid 1000 do usuario
# `node` da imagem e o mesmo do usuario comum no host, entao o que o zelador escreve nos
# volumes nasce com o dono certo. Para alcancar o docker.sock (root:docker no host), o
# compose entrega o gid do grupo docker por `group_add`.
#
# HOME propria: o claude guarda ~/.claude.json ao lado da CLAUDE_CONFIG_DIR, e apontar
# para o ~/.claude de uma pessoa deixa o agente achando o backup dela sem o arquivo --
# "Claude configuration file not found", em loop.
RUN mkdir -p /casa/.claude && chown -R node:node /casa
USER node
ENV HOME=/casa \
    CLAUDE_CONFIG_DIR=/casa/.claude
CMD ["zelador"]

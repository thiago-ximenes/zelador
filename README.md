# zelador

O agente que cuida da máquina, e você fala com ele pelo Telegram.

```
 seu chat ──► bot proprio ──► canal-telegram ──► caixa.tsv
                                                    │
                                      zelador (Rust, 632 KiB de plantao)
                                                    │
            ┌───────────────────┬───────────────────┴──────────────┐
            │                   │                                  │
     claude -p --resume    docker.sock                        ~/repos
     skills e memoria      ps, logs, restart, build, up    edita, testa, commita
```

Não é um bot de comandos. É o Claude Code rodando num laço: você escreve "o canal caiu,
vê o que aconteceu" e ele lê o log, decide e conserta — com o mesmo contexto que você tem
no terminal, porque monta as suas skills, a sua memória e os seus repos.

## Por que em Rust

O laço fica de pé 24 horas por dia sem fazer nada. Em Node isso custa ~13 MB de RSS
parado; aqui custa **632 KiB** — medido com `docker stats` no container ocioso. O
binário tem 623 KB.

Dito isso: **a imagem final tem 785 MB**, porque o `claude` é Node e precisa estar
dentro dela, e cada rodada dele gasta de 300 MB a 1 GB de RAM. O Rust economiza o
plantão, não o trabalho. Se alguém te disser que reescrever o wrapper em Rust corta o consumo do
agente, está errado.

## Como roda

```bash
cp exemplos/.env.exemplo exemplos/.env     # CLAUDE_TOKEN_1..N
cd exemplos && docker compose up -d        # o zelador + o canal do bot dele
```

| variável | o que faz |
|---|---|
| `CANAL_DIR` | onde o canal deixa `respostas/caixa.tsv` e lê `respostas/_saida/` |
| `ZELADOR_ESTADO` | marca de leitura e `session_id` (o fio da conversa) |
| `TRABALHO_DIR` | o `cwd` do `claude` — onde os repos estão |
| `CONTAS` | o `contas.json` com a folga de cada conta, compartilhado |
| `CLAUDE_TOKEN_1..9` | uma conta por linha; conta nova não pede rebuild |
| `ZELADOR_SKIP_PERMISSOES` | `1` roda com `--dangerously-skip-permissions` |
| `ZELADOR_TIMEOUT` | segundos por rodada (padrão 1500) |
| `ZELADOR_LEMBRETE` | arquivo com o bloco injetado em toda mensagem |

`zelador --estado` imprime a folga das contas sem gastar invocação.

## As contas do Claude

Várias contas, uma janela de 5h cada, e o token não diz quanta cota sobrou — então o
consumo é somado num arquivo e o 429 serve de rede de segurança, não de rotina.

Três regras que o laço respeita, e que custaram medição para existir:

**Fica na conta quente.** Trocar de conta joga fora o cache de prompt, que custa dezenas
de milhares de tokens de entrada por invocação. Só troca quando a folga aperta.

**A conta reservada não se encosta.** Se você marcou uma conta como sua, o zelador prefere
dizer "só sobrou a sua, não vou encostar" a atrapalhar o seu trabalho.

**O arquivo de contas é compartilhado.** Dois agentes com duas contabilidades inventam
folga que não existe e batem no limite no mesmo minuto.

## O que ele vê

O `docker.sock` montado dá ao zelador o Docker do host inteiro — na prática, root na
máquina. Isso é deliberado: um agente que só pode rodar cinco comandos de uma lista não
conserta nada que a lista não previu.

A consequência é que **a fronteira é a porta de entrada**: o bot é privado e o canal só
aceita mensagem do seu chat. Antes de subir isso, confira que o seu canal filtra o
remetente — um bot que reinicia container e aceita qualquer mensagem é um bot que
qualquer pessoa reinicia.

## Dois agentes, duas sessões

O `session_id` do zelador é dele. Compartilhar o fio com o agente que atende pessoa faz
você perguntar de container e receber o contexto da conversa de ontem.

MIT. Usa o [canal-telegram](https://github.com/thiago-ximenes/canal-telegram) como canal
de entrada e saída.

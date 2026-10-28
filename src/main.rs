// O zelador: o agente que cuida da maquina, falando pelo Telegram num bot proprio.
// Mesma ideia do agente que atende pessoa, outro papel -- aqui ninguem pede prep de
// entrevista: pede log, restart, build, espaco em disco, "por que o container caiu".
//
//   zelador                   -> laco, atende para sempre
//
// Entrada:  $CANAL_DIR/respostas/caixa.tsv       <- escrita pelo canal (canal-telegram)
// Saida:    $CANAL_DIR/respostas/_saida/<ts>.txt      <- o canal varre e envia
//                                      <ts>.voz.txt   <- sai como recado de voz
//                                      <ts>.json      <- lista de envios, formato escolhido
// Estado:   $ZELADOR_ESTADO/lido        <- ate que linha da caixa ja atendi
//           $ZELADOR_ESTADO/sessao.json <- session_id para o --resume manter o fio
//
// Em Rust porque fica de pe 24h sem fazer nada: o processo parado custa ~3 MB em vez dos
// ~13 MB do mesmo laco em Node. O que pesa de verdade e o `claude` de cada rodada, e isso
// nenhuma linguagem muda -- por isso o laco e burro de proposito: le, invoca, escreve.
mod contas;

use chrono::Local;
use contas::{Escolha, Pool};
use serde_json::Value;
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

// Vai em TODA mensagem: a sessao retomada por --resume nasceu antes da ultima regra e nao
// rele o CLAUDE.md do projeto. Sem isso o agente passa a tarde dizendo que nao sabe fazer
// o que ja sabe. Trocavel por arquivo em ZELADOR_LEMBRETE.
const LEMBRETE_PADRAO: &str = "\
[canal] voce responde pelo Telegram: tela de celular, leitura rapida.
Ate 6 linhas: responda em texto normal (markdown vira a marcacao do Telegram).
Saida de comando com colunas (docker ps, df -h): vira IMAGEM ou <pre>, nunca texto corrido.
Trabalho longo (build, deploy): tipo progresso com uma chave -- a mesma mensagem se atualiza.
Confirmar que fez, sem gastar linha: tipo reacao.
Para escolher o formato, grave a lista de envios em _saida/<ts>.json e responda [enviado].";

fn env_ou(chave: &str, padrao: &str) -> String {
    std::env::var(chave).unwrap_or_else(|_| padrao.to_string())
}

fn log(msg: &str) {
    println!("{} {}", Local::now().format("%Y-%m-%dT%H:%M:%S%.3fZ"), msg);
}

fn ler_num(arq: &Path) -> usize {
    fs::read_to_string(arq).ok().and_then(|t| t.trim().parse().ok()).unwrap_or(0)
}

fn agora_ms() -> u64 {
    contas::agora_ms()
}

/// O indicador de "digitando" do Telegram so acende enquanto ha trabalho de verdade:
/// quem acende e quem esta efetivamente rodando o claude, nao o laco.
fn trabalhando(arq: &Path, segundos: u64) {
    if segundos == 0 {
        fs::remove_file(arq).ok();
        return;
    }
    fs::write(arq, (agora_ms() + segundos * 1000).to_string()).ok();
}

/// "[voz]" na primeira linha e pedido de recado falado; o canal converte e o marcador nao
/// vai junto na fala. "[enviado]" e o caso em que o agente ja deixou o <ts>.json na fila e
/// escolheu os formatos -- repetir em texto duplicaria tudo no chat.
fn responder(saida: &Path, texto: &str) {
    let t = texto.trim_start();
    if t.to_lowercase().starts_with("[enviado]") {
        return;
    }
    let voz = t.to_lowercase().starts_with("[voz]");
    let corpo = if voz { t[5..].trim_start() } else { t };
    let nome = format!("{}{}.txt", agora_ms(), if voz { ".voz" } else { "" });
    fs::write(saida.join(nome), corpo).ok();
}

/// "resets at 14:30" dentro do erro de limite. Sem crate de regex: acha o primeiro
/// HH:MM depois da palavra reset.
fn achar_hora(txt: &str) -> Option<(u32, u32)> {
    let baixo = txt.to_lowercase();
    let desde = baixo.find("reset")? + 5;
    let bytes: Vec<char> = baixo[desde..].chars().collect();
    for i in 0..bytes.len() {
        if bytes[i] == ':' && i > 0 {
            let h: String = bytes[..i].iter().rev().take_while(|c| c.is_ascii_digit()).collect::<Vec<_>>().into_iter().rev().collect();
            let m: String = bytes[i + 1..].iter().take_while(|c| c.is_ascii_digit()).collect();
            if !h.is_empty() && m.len() >= 2 {
                if let (Ok(hh), Ok(mm)) = (h.parse::<u32>(), m[..2].parse::<u32>()) {
                    if hh < 24 && mm < 60 {
                        return Some((hh, mm));
                    }
                }
            }
        }
    }
    None
}

struct Conf {
    trabalho: String,
    estado: PathBuf,
    lembrete: String,
    limite_seg: u64,
    skip_permissoes: bool,
}

/// Uma rodada: escolhe a conta, roda o claude, cobra o uso. Em limite, marca a conta e
/// tenta a proxima; a conta reservada para o humano nao entra -- ele reservou porque esta
/// trabalhando nela, e sem clique de confirmacao a decisao e nao encostar.
fn atender(mensagem: &str, conf: &Conf, pool: &Pool) -> Result<String, String> {
    let sessao_arq = conf.estado.join("sessao.json");
    let sessao = fs::read_to_string(&sessao_arq)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("id").and_then(|i| i.as_str()).map(String::from));
    let prompt = format!("{}\n\n{}", mensagem, conf.lembrete);

    for _ in 0..pool.quantas().max(1) {
        let (n, token) = match pool.escolher(3) {
            Escolha::Conta(n, t) => (n, t),
            Escolha::SoAReservada(n) => {
                return Err(format!("so sobrou a conta {n}, que esta reservada para voce; nao vou encostar"))
            }
            Escolha::Nenhuma => return Err("todas as contas no limite".into()),
        };

        let mut args: Vec<String> = vec![
            "-p".into(),
            "--output-format".into(),
            "json".into(),
            // A janela e de 1M, mas deixar encher ate la custa caro e degrada a resposta.
            "--autocompact".into(),
            "200k".into(),
        ];
        if conf.skip_permissoes {
            args.push("--dangerously-skip-permissions".into());
        }
        if let Some(id) = &sessao {
            args.push("--resume".into());
            args.push(id.clone());
        }
        args.push(prompt.clone());

        // stdout em arquivo, nao em pipe: com pipe, uma resposta grande enche o buffer e
        // o processo trava esperando leitura -- e aqui ninguem le ate ele terminar.
        let fora = conf.estado.join("ultima-saida.json");
        let erro = conf.estado.join("ultimo-erro.txt");
        let mut filho = Command::new("claude")
            .args(&args)
            .current_dir(&conf.trabalho)
            .env("CLAUDE_CODE_OAUTH_TOKEN", &token)
            .stdout(Stdio::from(File::create(&fora).map_err(|e| e.to_string())?))
            .stderr(Stdio::from(File::create(&erro).map_err(|e| e.to_string())?))
            .spawn()
            .map_err(|e| format!("nao consegui rodar o claude: {e}"))?;

        let limite = agora_ms() + conf.limite_seg * 1000;
        let saiu = loop {
            match filho.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {
                    if agora_ms() > limite {
                        filho.kill().ok();
                        filho.wait().ok();
                        break None;
                    }
                    thread::sleep(Duration::from_millis(500));
                }
                Err(e) => return Err(format!("esperando o claude: {e}")),
            }
        };

        let texto_fora = fs::read_to_string(&fora).unwrap_or_default();
        let texto_erro = fs::read_to_string(&erro).unwrap_or_default();

        let Some(status) = saiu else {
            return Err(format!("passou de {} min e eu matei a rodada", conf.limite_seg / 60));
        };

        if status.success() {
            pool.cobrar(n, 1);
            let j: Value = serde_json::from_str(&texto_fora).unwrap_or(Value::Null);
            if let Some(id) = j.get("session_id").and_then(|s| s.as_str()) {
                fs::write(
                    &sessao_arq,
                    serde_json::json!({ "id": id, "quando": agora_ms() }).to_string(),
                )
                .ok();
            }
            return Ok(match j.get("result").and_then(|r| r.as_str()) {
                Some(r) => r.to_string(),
                None => texto_fora,
            });
        }

        let tudo = format!("{texto_erro}{texto_fora}").to_lowercase();
        if tudo.contains("limit reached") || tudo.contains("rate limit") || tudo.contains("429") {
            let hora = achar_hora(&tudo);
            pool.marcar_cheia(n, hora);
            log(&format!("conta {n} no limite; tentando a proxima"));
            continue;
        }
        let pedaco: String = texto_erro.chars().take(300).collect();
        return Err(format!("conta {n} falhou: {pedaco}"));
    }
    Err("todas as contas no limite".into())
}

fn main() {
    let canal = PathBuf::from(env_ou("CANAL_DIR", "/estado"));
    let respostas = canal.join("respostas");
    let caixa = respostas.join("caixa.tsv");
    let saida = respostas.join("_saida");
    let indicador = respostas.join(".trabalhando");
    let estado = PathBuf::from(env_ou("ZELADOR_ESTADO", "/estado-zelador"));
    // Prefixo de id de botao que e DELE: o canal devolve a escolha com o id de quem
    // perguntou, e clique de pergunta de outro processo nao e assunto deste laco.
    let prefixo = env_ou("ZELADOR_PREFIXO", "ze-");
    let conf = Conf {
        trabalho: env_ou("TRABALHO_DIR", "/trabalho"),
        estado: estado.clone(),
        lembrete: std::env::var("ZELADOR_LEMBRETE")
            .ok()
            .and_then(|p| fs::read_to_string(p).ok())
            .unwrap_or_else(|| LEMBRETE_PADRAO.to_string()),
        limite_seg: env_ou("ZELADOR_TIMEOUT", "1500").parse().unwrap_or(1500),
        skip_permissoes: env_ou("ZELADOR_SKIP_PERMISSOES", "0") == "1",
    };
    fs::create_dir_all(&saida).ok();
    fs::create_dir_all(&estado).ok();

    let pool = Pool::nova();
    // `zelador --estado` responde a folga de cada conta e sai: serve para conferir o
    // palpite de cota sem gastar uma invocacao.
    if std::env::args().any(|a| a == "--estado") {
        println!("{}", pool.estado());
        return;
    }
    if pool.quantas() == 0 {
        log("nenhum CLAUDE_TOKEN_* no ambiente -- rode `claude setup-token` e preencha o .env");
        std::process::exit(1);
    }
    let marca = estado.join("lido");
    log(&format!(
        "zelador de plantao: {} conta(s), trabalho em {}, permissoes {}",
        pool.quantas(),
        conf.trabalho,
        if conf.skip_permissoes { "puladas" } else { "normais" }
    ));

    loop {
        if let Ok(txt) = fs::read_to_string(&caixa) {
            let linhas: Vec<&str> = txt.lines().filter(|l| !l.trim().is_empty()).collect();
            let lidas = ler_num(&marca);
            for i in lidas..linhas.len() {
                let campos: Vec<&str> = linhas[i].split('\t').collect();
                // A marca sobe ANTES de atender: rodada que falha nao volta na proxima
                // varredura. Mensagem perdida e melhor que a mesma falha para sempre.
                fs::write(&marca, (i + 1).to_string()).ok();
                let id = campos.get(1).copied().unwrap_or("-").trim();
                let texto = campos.get(2).copied().unwrap_or("").trim();
                let meu_botao = id.starts_with(&prefixo);
                // id != "-" ja e resposta de uma pergunta de outro processo; "/" e comando
                // que o proprio canal respondeu.
                if (id != "-" && !meu_botao) || texto.is_empty() || texto.starts_with('/') {
                    continue;
                }
                let resumo: String = texto.chars().take(60).collect();
                log(&format!("atendendo: {}{}", if meu_botao { "[botao] " } else { "" }, resumo));
                trabalhando(&indicador, conf.limite_seg);
                match atender(texto, &conf, &pool) {
                    Ok(r) => responder(&saida, &r),
                    Err(e) => {
                        log(&format!("falhou: {e}"));
                        responder(&saida, &format!("nao consegui: {e}"));
                    }
                }
                trabalhando(&indicador, 0);
            }
        }
        thread::sleep(Duration::from_secs(3));
    }
}

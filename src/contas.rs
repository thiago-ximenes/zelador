// A regra de folga das contas do Claude, portada do roteador em Node que ja roda no
// zezinho. O estado e o MESMO arquivo (contas.json no volume compartilhado), de proposito:
// dois agentes com duas contabilidades inventariam folga que nao existe e levariam os dois
// ao 429 no mesmo minuto.
//
// Formato, escrito pelos dois lados:
//   { "1": { "abriu": <ms>, "usos": 7, "bloqueadaAte": <ms opcional> },
//     "ativa": 1,                      <- a conta quente, para nao jogar fora o cache
//     "ocupada": { "conta": 2, ... },  <- reservada para o humano; nao se encosta
//     "contas": [1,2,3], "rotulos": { "1": "..." } }
use chrono::{Duration as CDuration, Local, Timelike};
use serde_json::{json, Map, Value};
use std::{fs, path::PathBuf, time::{SystemTime, UNIX_EPOCH}};

pub const JANELA_MS: u64 = 5 * 60 * 60 * 1000;

pub fn agora_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}

pub enum Escolha {
    Conta(u32, String),
    /// So sobrou a conta reservada para o humano. Sem clique de confirmacao, a decisao
    /// e nao encostar: ele reservou porque esta trabalhando nela.
    SoAReservada(u32),
    Nenhuma,
}

pub struct Pool {
    arq: PathBuf,
    teto: u32,
    tokens: Vec<(u32, String)>,
}

impl Pool {
    pub fn nova() -> Pool {
        let arq = PathBuf::from(
            std::env::var("CONTAS").unwrap_or_else(|_| "/compartilhado/contas.json".into()),
        );
        let teto: u32 = std::env::var("TETO").ok().and_then(|t| t.parse().ok()).unwrap_or(860);
        // CLAUDE_TOKEN_1..9: conta nova e uma linha nova no .env, sem rebuild.
        let tokens: Vec<(u32, String)> = (1..=9)
            .filter_map(|n| std::env::var(format!("CLAUDE_TOKEN_{n}")).ok().map(|t| (n, t)))
            .filter(|(_, t)| !t.trim().is_empty())
            .collect();
        Pool { arq, teto, tokens }
    }

    pub fn quantas(&self) -> usize {
        self.tokens.len()
    }

    fn ler(&self) -> Map<String, Value> {
        fs::read_to_string(&self.arq)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default()
    }

    fn gravar(&self, e: &Map<String, Value>) {
        if let Some(pai) = self.arq.parent() {
            fs::create_dir_all(pai).ok();
        }
        if let Ok(txt) = serde_json::to_string_pretty(&Value::Object(e.clone())) {
            fs::write(&self.arq, txt).ok();
        }
    }

    /// Zera a conta cuja janela de 5h fechou. A reserva do humano NAO expira por relogio:
    /// so sai quando ele soltar. Sao duas coisas sem relacao.
    fn limpar(&self, e: &mut Map<String, Value>) {
        let agora = agora_ms();
        let vencidas: Vec<String> = e
            .iter()
            .filter(|(k, v)| {
                k.chars().all(|c| c.is_ascii_digit())
                    && v.get("abriu").and_then(|a| a.as_u64()).map_or(false, |a| agora.saturating_sub(a) > JANELA_MS)
            })
            .map(|(k, _)| k.clone())
            .collect();
        for k in vencidas {
            e.remove(&k);
        }
    }

    fn folga(&self, e: &Map<String, Value>, n: u32) -> u32 {
        let Some(s) = e.get(&n.to_string()) else { return self.teto };
        if let Some(ate) = s.get("bloqueadaAte").and_then(|b| b.as_u64()) {
            if agora_ms() < ate {
                return 0;
            }
        }
        let usos = s.get("usos").and_then(|u| u.as_u64()).unwrap_or(0) as u32;
        self.teto.saturating_sub(usos)
    }

    /// Fica na conta quente ate ela apertar: trocar joga fora o cache de prompt, que custa
    /// dezenas de milhares de tokens de entrada por invocacao. Tarefa grande vai para quem
    /// tem folga para ela inteira.
    pub fn escolher(&self, precisa: u32) -> Escolha {
        let mut e = self.ler();
        self.limpar(&mut e);
        let dele = e.get("ocupada").and_then(|o| o.get("conta")).and_then(|c| c.as_u64()).map(|c| c as u32);
        let livres: Vec<&(u32, String)> = self
            .tokens
            .iter()
            .filter(|(n, _)| Some(*n) != dele && self.folga(&e, *n) >= precisa)
            .collect();
        if livres.is_empty() {
            return match dele {
                Some(d) if self.folga(&e, d) >= precisa => Escolha::SoAReservada(d),
                _ => Escolha::Nenhuma,
            };
        }
        let ativa = e.get("ativa").and_then(|a| a.as_u64()).map(|a| a as u32);
        if let Some(q) = livres.iter().find(|(n, _)| Some(*n) == ativa) {
            if self.folga(&e, q.0) >= precisa.saturating_mul(2) {
                return Escolha::Conta(q.0, q.1.clone());
            }
        }
        let melhor = livres.iter().max_by_key(|(n, _)| self.folga(&e, *n)).unwrap();
        Escolha::Conta(melhor.0, melhor.1.clone())
    }

    pub fn cobrar(&self, n: u32, usos: u32) {
        let mut e = self.ler();
        self.limpar(&mut e);
        let chave = n.to_string();
        let anterior = e.get(&chave).cloned();
        let abriu = anterior.as_ref().and_then(|s| s.get("abriu")).and_then(|a| a.as_u64()).unwrap_or_else(agora_ms);
        let ja = anterior.as_ref().and_then(|s| s.get("usos")).and_then(|u| u.as_u64()).unwrap_or(0);
        let mut novo = json!({ "abriu": abriu, "usos": ja + usos as u64 });
        if let Some(ate) = anterior.as_ref().and_then(|s| s.get("bloqueadaAte")).and_then(|b| b.as_u64()) {
            novo["bloqueadaAte"] = json!(ate);
        }
        e.insert(chave, novo);
        e.insert("ativa".into(), json!(n));
        self.gravar(&e);
    }

    /// O token nao diz quanta cota sobrou, entao o 429 e a unica fonte da verdade quando
    /// o palpite erra: a conta vai a teto e, se a mensagem trouxe "resets at 14:30",
    /// fica bloqueada ate aquela hora -- senao a proxima invocacao a escolhe de novo.
    pub fn marcar_cheia(&self, n: u32, reset: Option<(u32, u32)>) {
        let mut e = self.ler();
        let chave = n.to_string();
        let abriu = e.get(&chave).and_then(|s| s.get("abriu")).and_then(|a| a.as_u64()).unwrap_or_else(agora_ms);
        let mut s = json!({ "abriu": abriu, "usos": self.teto });
        if let Some((h, m)) = reset {
            let agora = Local::now();
            let mut quando = agora.with_hour(h).and_then(|d| d.with_minute(m)).and_then(|d| d.with_second(0));
            if let Some(d) = quando {
                if d < agora {
                    quando = Some(d + CDuration::days(1));
                }
            }
            if let Some(d) = quando {
                s["bloqueadaAte"] = json!(d.timestamp_millis() as u64);
            }
        }
        e.insert(chave, s);
        self.gravar(&e);
    }

    pub fn estado(&self) -> String {
        let mut e = self.ler();
        self.limpar(&mut e);
        let dele = e.get("ocupada").and_then(|o| o.get("conta")).and_then(|c| c.as_u64()).map(|c| c as u32);
        self.tokens
            .iter()
            .map(|(n, _)| {
                let marca = if Some(*n) == dele { "  <- reservada" } else { "" };
                format!("conta {}: {}/{} livres{}", n, self.folga(&e, *n), self.teto, marca)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

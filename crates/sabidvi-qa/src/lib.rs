//! 契約 case の台帳（SabiSeries の品質保証指針 `qa-v0`、証拠の状態 PASS / FAIL / BLOCKED / NOT-RUN）。
//!
//! oracle テストは case を一つ作り、
//!
//! - 参照環境（ツール・フォント）が無ければ `blocked(理由)`（BLOCKED）。必須 case は `SABI_STRICT_TESTS` で失敗、
//!   任意 case は `SABI_STRICT_OPTIONAL` も設定されているときだけ失敗にする。任意かどうかは case の属性で、理由の文字列から推測しない
//! - 参照ツールは起動できたが失敗した（終了コード、出力ファイルの欠落）なら `tool_failed(理由)`（FAIL。常に失敗）
//! - 比較を一つ終えるたびに `compared()`、全部終えたら `done()`（PASS）
//!
//! `done()` も `blocked()` も呼ばずに終わった case は NOT-RUN として記録し、strict では失敗にする
//! （`if let Some` や `continue` で比較を黙って飛ばす経路を捕まえる）。
//!
//! 台帳は環境変数 `SABI_QA_LEDGER` のディレクトリ（無ければ `target/qa-ledger`）に、テスト実行ごとに 1 行ずつ追記する。
//! 形式: `<case-id>\t<STATUS>\tcomparisons=<n>\t<contracts>\t<optional|required>\t<理由>`。
//! `specification/cases.md` の予定 case と突き合わせるのは `scripts/qa-ledger.sh`。

use std::cell::Cell;
use std::io::Write;
use std::path::PathBuf;

pub struct Case {
    id: &'static str,
    contracts: &'static [&'static str],
    optional: bool,
    comparisons: Cell<usize>,
    finished: Cell<bool>,
}

fn strict() -> bool {
    std::env::var_os("SABI_STRICT_TESTS").is_some()
}

fn strict_optional() -> bool {
    std::env::var_os("SABI_STRICT_OPTIONAL").is_some()
}

fn ledger_dir() -> PathBuf {
    match std::env::var_os("SABI_QA_LEDGER") {
        Some(d) => PathBuf::from(d),
        // ワークスペースの target/qa-ledger（このクレートの位置からの相対。テストの cwd はパッケージごとに違う）
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("target")
            .join("qa-ledger"),
    }
}

fn record(line: &str) {
    let dir = ledger_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let file = dir.join(format!("{}.tsv", env!("CARGO_PKG_NAME")));
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
    {
        let _ = writeln!(f, "{line}");
    }
}

impl Case {
    pub fn required(id: &'static str, contracts: &'static [&'static str]) -> Case {
        Case::new(id, contracts, false)
    }

    /// 任意の参照環境（追加フォント、別エンジン）に依る case。台帳では optional と明示する
    pub fn optional(id: &'static str, contracts: &'static [&'static str]) -> Case {
        Case::new(id, contracts, true)
    }

    fn new(id: &'static str, contracts: &'static [&'static str], optional: bool) -> Case {
        Case {
            id,
            contracts,
            optional,
            comparisons: Cell::new(0),
            finished: Cell::new(false),
        }
    }

    pub fn id(&self) -> &'static str {
        self.id
    }

    fn line(&self, status: &str, reason: &str) -> String {
        format!(
            "{}\t{}\tcomparisons={}\t{}\t{}\t{}",
            self.id,
            status,
            self.comparisons.get(),
            self.contracts.join(","),
            if self.optional {
                "optional"
            } else {
                "required"
            },
            reason.replace(['\t', '\n'], " ")
        )
    }

    /// 参照環境が無い。必須 case は strict で失敗、任意 case は strict + optional で失敗
    pub fn blocked(&self, reason: &str) {
        self.finished.set(true);
        record(&self.line("BLOCKED", reason));
        let fail = strict() && (!self.optional || strict_optional());
        if fail {
            panic!(
                "{}: required reference environment is missing: {reason}",
                self.id
            );
        }
        eprintln!(
            "{}: BLOCKED{}: {reason}",
            self.id,
            if self.optional { " (optional)" } else { "" }
        );
    }

    /// 参照ツールは在るのに失敗した（終了コード、出力の欠落）。環境不足ではなく検査の失敗
    pub fn tool_failed(&self, reason: &str) -> ! {
        self.finished.set(true);
        record(&self.line("FAIL", reason));
        panic!("{}: reference tool failed: {reason}", self.id)
    }

    /// 比較を一つ終えた
    pub fn compared(&self) {
        self.comparisons.set(self.comparisons.get() + 1);
    }

    pub fn compared_n(&self, n: usize) {
        self.comparisons.set(self.comparisons.get() + n);
    }

    /// 予定した比較をすべて終えた。比較が一つも無ければ NOT-RUN 扱いで失敗
    pub fn done(&self) {
        self.finished.set(true);
        if self.comparisons.get() == 0 {
            record(&self.line("NOT-RUN", "done() without any comparison"));
            panic!("{}: finished without any comparison", self.id);
        }
        record(&self.line("PASS", ""));
    }
}

impl Drop for Case {
    fn drop(&mut self) {
        if self.finished.get() {
            return;
        }
        if std::thread::panicking() {
            // assertion に落ちた: 失敗として記録する
            record(&self.line("FAIL", "assertion failed"));
            return;
        }
        record(&self.line("NOT-RUN", "returned before done() or blocked()"));
        if strict() {
            panic!(
                "{}: test returned without completing its comparisons",
                self.id
            );
        }
        eprintln!("{}: NOT-RUN (returned early)", self.id);
    }
}

/// 参照ツールを実行する。起動できなければ None（BLOCKED の判断は呼び出し側）、起動して失敗すれば `tool_failed`
pub fn run_tool(
    case: &Case,
    program: &str,
    args: &[&str],
    cwd: Option<&std::path::Path>,
) -> Option<std::process::Output> {
    let mut cmd = std::process::Command::new(program);
    cmd.args(args);
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    match cmd.output() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => case.tool_failed(&format!("{program}: {e}")),
        Ok(out) => {
            if !out.status.success() {
                case.tool_failed(&format!(
                    "{program} {} exited with {}: {}",
                    args.join(" "),
                    out.status,
                    String::from_utf8_lossy(&out.stderr).trim()
                ));
            }
            Some(out)
        }
    }
}

/// `kpsewhich` でファイルを探す。kpsewhich 自体が無ければ None、無いファイルも None
pub fn kpsewhich(name: &str) -> Option<PathBuf> {
    let out = std::process::Command::new("kpsewhich")
        .arg(name)
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then(|| PathBuf::from(s))
}

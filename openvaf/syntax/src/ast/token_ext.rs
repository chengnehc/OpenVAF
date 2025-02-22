use crate::ast;

impl ast::IntNumber {
    pub fn value(&self) -> i32 {
        self.syntax.text().parse().unwrap()
    }
}

impl ast::StrLit {
    pub fn value(&self) -> &str {
        let src = self.syntax.text();
        &src[1..src.len() - 1]
    }
    pub fn unescaped_value(&self) -> String {
        self.value()
            .replace(r"\n", "\n")
            .replace(r"\\", "\\")
            .replace(r"\t", "\t")
            .replace(r#"\""#, "\"")
            .replace("\\\n", "\n")
            .replace("\\\r\n", "\r\n")
    }
}

impl ast::StdRealNumber {
    pub fn value(&self) -> f64 {
        self.syntax.text().parse().unwrap()
    }
}

impl ast::SiRealNumber {
    pub fn value(&self) -> f64 {
        let src = self.syntax.text();
        let (src, scale_char) = src.split_at(src.len() - 1);
        let exp = match scale_char {
            "T" => 12,
            "G" => 9,
            "M" => 6,
            "K" | "k" => 3,
            "m" => -3,
            "u" => -6,
            "n" => -9,
            "p" => -12,
            "f" => -15,
            "a" => -18,
            _ => unreachable!(),
        };
        src.parse::<f64>().unwrap() * (10_f64).powi(exp)
    }
}

//! Bounded arithmetic with exact rational values; no executable expressions.
use anyhow::{Context, Result, bail, ensure};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Number(i128, i128);

impl Number {
    fn new(n: i128, d: i128) -> Result<Self> {
        ensure!(d != 0, "算式不能除以零");
        let gcd = |mut a: u128, mut b: u128| {
            while b != 0 {
                (a, b) = (b, a % b);
            }
            a.max(1) as i128
        };
        let g = gcd(n.unsigned_abs(), d.unsigned_abs());
        let n = n / g;
        let d = d / g;
        if d < 0 {
            Ok(Self(n.checked_neg().ok_or_else(overflow)?, d.checked_neg().ok_or_else(overflow)?))
        } else {
            Ok(Self(n, d))
        }
    }
    pub fn decimal(text: &str) -> Result<Self> {
        let (whole, decimal) = text.split_once('.').unwrap_or((text, ""));
        ensure!(decimal.len() <= 12, "小数精度超过限制");
        let sign = if whole.starts_with('-') { -1 } else { 1 };
        let d = 10i128.checked_pow(decimal.len() as u32).ok_or_else(overflow)?;
        let n = whole
            .parse::<i128>()?
            .checked_mul(d)
            .ok_or_else(overflow)?
            .checked_add(sign * if decimal.is_empty() { 0 } else { decimal.parse::<i128>()? })
            .ok_or_else(overflow)?;
        Self::new(n, d)
    }
    fn op(self, op: char, other: Self) -> Result<Self> {
        let mul = |a: i128, b: i128| a.checked_mul(b).ok_or_else(overflow);
        match op {
            '+' | '-' => {
                let a = mul(self.0, other.1)?;
                let b = mul(other.0, self.1)?;
                let n = if op == '+' { a.checked_add(b) } else { a.checked_sub(b) }.ok_or_else(overflow)?;
                Self::new(n, mul(self.1, other.1)?)
            }
            '*' | '×' => Self::new(mul(self.0, other.0)?, mul(self.1, other.1)?),
            '/' | '÷' => Self::new(mul(self.0, other.1)?, mul(self.1, other.0)?),
            _ => bail!("不支持的算符"),
        }
    }
    pub fn scale(self, factor: i128) -> Result<Self> {
        self.op('*', Self(factor, 1))
    }
    fn display(self) -> Result<String> {
        let n = self.0.unsigned_abs();
        let d = self.1 as u128;
        let mut result = format!("{}{}", if self.0 < 0 { "-" } else { "" }, n / d);
        let mut remainder = n % d;
        if remainder != 0 {
            result.push('.');
        }
        for _ in 0..12 {
            if remainder == 0 {
                break;
            }
            let next = remainder.checked_mul(10).ok_or_else(overflow)?;
            result.push(char::from(b'0' + (next / d) as u8));
            remainder = next % d;
        }
        ensure!(remainder == 0, "计算结果不能精确表示为12位内的小数，请调整单位或使用分数说明");
        Ok(result)
    }
}

fn overflow() -> anyhow::Error {
    anyhow::anyhow!("算式数值超过限制")
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
    money: bool,
}
impl Parser {
    fn peek(&mut self) -> Option<char> {
        while self.chars.get(self.pos).is_some_and(|c| c.is_whitespace()) {
            self.pos += 1;
        }
        self.chars.get(self.pos).copied()
    }
    fn expr(&mut self, depth: usize) -> Result<Number> {
        ensure!(depth <= 16, "算式括号过深");
        let mut value = self.term(depth)?;
        while let Some(op @ ('+' | '-')) = self.peek() {
            self.pos += 1;
            value = value.op(op, self.term(depth)?)?;
        }
        Ok(value)
    }
    fn term(&mut self, depth: usize) -> Result<Number> {
        let mut value = self.factor(depth)?;
        while let Some(op @ ('*' | '×' | '/' | '÷')) = self.peek() {
            self.pos += 1;
            value = value.op(op, self.factor(depth)?)?;
        }
        Ok(value)
    }
    fn factor(&mut self, depth: usize) -> Result<Number> {
        let mut sign = 1;
        if let Some(c @ ('+' | '-')) = self.peek() {
            if c == '-' {
                sign = -1;
            }
            self.pos += 1;
        }
        let value = if self.peek() == Some('(') {
            self.pos += 1;
            let value = self.expr(depth + 1)?;
            ensure!(self.peek() == Some(')'), "算式缺少右括号");
            self.pos += 1;
            value
        } else {
            self.peek();
            let start = self.pos;
            while self.chars.get(self.pos).is_some_and(|c| c.is_ascii_digit() || *c == '.') {
                self.pos += 1;
            }
            ensure!(start != self.pos, "算式缺少数字");
            let value = Number::decimal(&self.chars[start..self.pos].iter().collect::<String>())?;
            self.peek();
            let tail = self.chars[self.pos..].iter().collect::<String>();
            let mut value = value;
            // Currency conversion is exact. Counts/durations are suffixes; rate
            // denominators are kept as units, not mistaken for numeric division.
            for (unit, scale) in [("亿元", 100_000_000), ("万元", 10_000), ("元", 1)] {
                if tail.starts_with(unit) {
                    self.pos += unit.chars().count();
                    self.money = true;
                    value = value.scale(scale)?;
                    break;
                }
            }
            let tail = self.chars[self.pos..].iter().collect::<String>();
            for unit in [
                "/副本/月",
                "/副本/年",
                "/个/月",
                "/人/月",
                "/月",
                "/年",
                "个副本",
                "个月",
                "副本",
                "毫秒",
                "小时",
                "分钟",
                "个",
                "月",
                "年",
                "天",
                "秒",
            ] {
                if tail.starts_with(unit) {
                    self.pos += unit.chars().count();
                    break;
                }
            }
            value
        };
        value.scale(sign)
    }
}

fn evaluate(text: &str) -> Result<(Number, bool)> {
    ensure!(text.chars().count() <= 512, "算式过长");
    let mut parser = Parser { chars: text.chars().collect(), pos: 0, money: false };
    let value = parser.expr(0)?;
    ensure!(parser.peek().is_none(), "算式包含无法解析的单位或字符");
    Ok((value, parser.money))
}

pub(super) fn result(expression: &str) -> Result<String> {
    ensure!(
        expression.chars().count() <= 256
            && !expression.trim().is_empty()
            && expression.chars().all(|c| c.is_ascii_digit() || c.is_whitespace() || ".+-*/()×÷".contains(c)),
        "calculation.expression只允许数字、四则算符和括号，不可夹入字段名、单位或等号"
    );
    evaluate(expression)?.0.display()
}

pub(super) struct Equation {
    pub result: Number,
    pub raw_result: Number,
    pub explicit_currency: bool,
}

pub(super) fn equations(text: &str) -> Result<Vec<Equation>> {
    let text = text
        .replace("乘以", "*")
        .replace("乘", "*")
        .replace("除以", "/")
        .replace("加上", "+")
        .replace("减去", "-")
        .replace("等于", "=");
    static EQUATION: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
        regex::Regex::new(r"((?:[+\-(]\s*)*\d[\d.\s+*/×÷()\-元万亿副本个月年天人毫秒小时分钟]*?)[=＝]\s*(-?\d+(?:\.\d+)?)\s*([万亿]?元)?").unwrap()
    });
    let mut results = Vec::new();
    for c in EQUATION.captures_iter(&text) {
        if !c[1].contains(['+', '-', '*', '/', '×', '÷']) {
            continue;
        }
        let (actual, has_currency) = evaluate(c[1].trim())
            .with_context(|| format!("无法解析算式：{}；请使用calculation字段提供纯数字表达式", &c[1]))?;
        let raw_result = Number::decimal(&c[2])?;
        let unit = c.get(3).map(|m| m.as_str());
        let scale = match unit {
            Some("万元") => 10_000,
            Some("亿元") => 100_000_000,
            _ => 1,
        };
        let result = raw_result.scale(scale)?;
        // Unitless operands inherit the displayed result unit (112+36=148万元).
        let expected = if has_currency { result } else { raw_result };
        ensure!(actual == expected, "计算结果错误：{}，请重新计算并保留来源和单位", &c[0]);
        results.push(Equation { result, raw_result, explicit_currency: unit.is_some() });
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_complete_chains_units_and_decimal_arithmetic() {
        for text in [
            "总费1500*7*3=31500元",
            "净额1500×7×3-600=30900元",
            "余量33300-(1500*7*3-600)=2400元",
            "每月1500元/副本/月 × 7副本 = 10500元",
            "1500元/副本/月*7副本*3个月=31500元",
            "1万元+5000元=15000元",
            "112+36=148万元",
            "0.1+0.2=0.3元",
            "9/3+2*4=11",
            "(-2+5)*3=9",
            "每月费用为1500乘以7等于10500元",
        ] {
            assert_eq!(equations(text).unwrap_or_else(|e| panic!("{text}: {e}")).len(), 1, "{text}");
        }
    }
    #[test]
    fn rejects_wrong_results_division_zero_and_resource_exhaustion() {
        for text in ["1500*7*3=21000元", "0.1+0.2=0.4元", "1/0=1", "(1+2=3", "1万元+5000元=6000元"] {
            assert!(equations(text).is_err(), "{text}");
        }
        assert!(evaluate(&format!("{}1{}", "(".repeat(20), ")".repeat(20))).is_err());
        assert!(evaluate("999999999999999999999999*999999999999999999999999").is_err());
        assert!(evaluate("1+system(2)").is_err());
        assert_eq!(result("(40*3)*2-5").unwrap(), "235");
        assert_eq!(result("-0.1+0.3").unwrap(), "0.2");
        assert!(result("1/3").is_err());
        assert!(result("40元*3").is_err());
    }
}

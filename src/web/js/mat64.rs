//! Matematica de f64 para o objeto Math (sem libm no kernel): reducao de
//! faixa + series, com precisao perto do ultimo bit na faixa util.

#[allow(unused_imports)]
use self::F64Ext as _;
const LN2: f64 = core::f64::consts::LN_2;
const PI: f64 = core::f64::consts::PI;
const PI_2: f64 = core::f64::consts::FRAC_PI_2;

pub fn floor(x: f64) -> f64 {
    if !x.is_finite() || x.abs() >= 4503599627370496.0 {
        return x;
    }
    let i = x as i64 as f64;
    if i > x { i - 1.0 } else { i }
}

pub fn ceil(x: f64) -> f64 {
    if !x.is_finite() || x.abs() >= 4503599627370496.0 {
        return x;
    }
    let i = x as i64 as f64;
    if i < x { i + 1.0 } else { i }
}

pub fn trunc(x: f64) -> f64 {
    if !x.is_finite() || x.abs() >= 4503599627370496.0 {
        return x;
    }
    x as i64 as f64
}

/// Math.round: metade vai para +infinito.
pub fn round(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let f = floor(x);
    if x - f >= 0.5 { f + 1.0 } else if f == 0.0 && x < 0.0 { -0.0 } else { f }
}

pub fn sqrt(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 || x.is_infinite() {
        return x;
    }
    let mut y = f64::from_bits((x.to_bits() >> 1) + 0x1FF8_0000_0000_0000);
    for _ in 0..6 {
        y = 0.5 * (y + x / y);
    }
    y
}

/// 2^k (k inteiro) direto nos bits.
fn pot2(k: i64) -> f64 {
    if k > 1023 {
        return f64::INFINITY;
    }
    if k < -1074 {
        return 0.0;
    }
    if k < -1022 {
        return f64::from_bits(1u64 << (k + 1074));
    }
    f64::from_bits(((k + 1023) as u64) << 52)
}

pub fn exp(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    if x > 709.8 {
        return f64::INFINITY;
    }
    if x < -745.2 {
        return 0.0;
    }
    let k = round(x / LN2);
    let r = x - k * LN2;
    // Taylor ate r^14 (|r| <= 0.35)
    let mut termo = 1.0;
    let mut soma = 1.0;
    for n in 1..18 {
        termo *= r / n as f64;
        soma += termo;
    }
    let k = k as i64;
    if k < -1020 {
        return soma * pot2(k + 60) * pot2(-60);
    }
    soma * pot2(k)
}

pub fn ln(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return f64::NEG_INFINITY;
    }
    if x.is_infinite() {
        return x;
    }
    // x = m * 2^e com m em [1, 2)
    let mut bits = x.to_bits();
    let mut e = ((bits >> 52) & 0x7FF) as i64;
    if e == 0 {
        // subnormal
        let y = x * pot2(60);
        bits = y.to_bits();
        e = ((bits >> 52) & 0x7FF) as i64 - 60;
    }
    let mut m = f64::from_bits((bits & 0x000F_FFFF_FFFF_FFFF) | 0x3FF0_0000_0000_0000);
    let mut e = e - 1023;
    if m > core::f64::consts::SQRT_2 {
        m /= 2.0;
        e += 1;
    }
    let s = (m - 1.0) / (m + 1.0);
    let s2 = s * s;
    let mut termo = s;
    let mut soma = 0.0;
    let mut n = 1.0;
    for _ in 0..30 {
        soma += termo / n;
        termo *= s2;
        n += 2.0;
    }
    2.0 * soma + e as f64 * LN2
}

pub fn pow(x: f64, y: f64) -> f64 {
    if y == 0.0 {
        return 1.0;
    }
    if x.is_nan() || y.is_nan() {
        return f64::NAN;
    }
    if x == 1.0 {
        return 1.0;
    }
    if x < 0.0 {
        if y.fract() != 0.0 {
            return f64::NAN;
        }
        let r = exp(y * ln(-x));
        return if (y as i64) % 2 != 0 { -r } else { r };
    }
    if x == 0.0 {
        return if y > 0.0 { 0.0 } else { f64::INFINITY };
    }
    exp(y * ln(x))
}

/// seno/cosseno em [-pi/4, pi/4]
fn sen_k(x: f64) -> f64 {
    let x2 = x * x;
    let mut termo = x;
    let mut soma = x;
    for n in 1..12 {
        termo *= -x2 / ((2 * n) as f64 * (2 * n + 1) as f64);
        soma += termo;
    }
    soma
}

fn cos_k(x: f64) -> f64 {
    let x2 = x * x;
    let mut termo = 1.0;
    let mut soma = 1.0;
    for n in 1..12 {
        termo *= -x2 / ((2 * n - 1) as f64 * (2 * n) as f64);
        soma += termo;
    }
    soma
}

/// (quadrante, resto) de x / (pi/2)
fn reduzir(x: f64) -> (i64, f64) {
    let k = round(x / PI_2);
    // pi/2 em duas partes (Cody-Waite)
    let a = 1.5707963267341256e+00;
    let b = 6.0771005065061922e-11;
    let c = 2.0222662487959506e-21;
    (k as i64, ((x - k * a) - k * b) - k * c)
}

pub fn sin(x: f64) -> f64 {
    if !x.is_finite() {
        return f64::NAN;
    }
    let (q, r) = reduzir(x);
    match q.rem_euclid(4) {
        0 => sen_k(r),
        1 => cos_k(r),
        2 => -sen_k(r),
        _ => -cos_k(r),
    }
}

pub fn cos(x: f64) -> f64 {
    if !x.is_finite() {
        return f64::NAN;
    }
    let (q, r) = reduzir(x);
    match q.rem_euclid(4) {
        0 => cos_k(r),
        1 => -sen_k(r),
        2 => -cos_k(r),
        _ => sen_k(r),
    }
}

pub fn tan(x: f64) -> f64 {
    sin(x) / cos(x)
}

pub fn atan(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    if x < 0.0 {
        return -atan(-x);
    }
    if x > 1.0 {
        return PI_2 - atan(1.0 / x);
    }
    // duas reducoes pela metade do angulo, depois serie
    let mut y = x;
    let mut fator = 1.0;
    for _ in 0..2 {
        y /= 1.0 + sqrt(1.0 + y * y);
        fator *= 2.0;
    }
    let y2 = y * y;
    let mut termo = y;
    let mut soma = 0.0;
    let mut n = 1.0;
    for k in 0..25 {
        let t = termo / n;
        soma += if k % 2 == 0 { t } else { -t };
        termo *= y2;
        n += 2.0;
    }
    fator * soma
}

pub fn atan2(y: f64, x: f64) -> f64 {
    if x.is_nan() || y.is_nan() {
        return f64::NAN;
    }
    if x > 0.0 {
        atan(y / x)
    } else if x < 0.0 {
        if y >= 0.0 { atan(y / x) + PI } else { atan(y / x) - PI }
    } else if y > 0.0 {
        PI_2
    } else if y < 0.0 {
        -PI_2
    } else {
        0.0
    }
}

pub fn asin(x: f64) -> f64 {
    if !(-1.0..=1.0).contains(&x) {
        return f64::NAN;
    }
    atan2(x, sqrt(1.0 - x * x))
}

pub fn acos(x: f64) -> f64 {
    if !(-1.0..=1.0).contains(&x) {
        return f64::NAN;
    }
    atan2(sqrt(1.0 - x * x), x)
}

pub fn cbrt(x: f64) -> f64 {
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    let r = exp(ln(x.abs()) / 3.0);
    // um passo de Newton para acertar o ultimo digito
    let r = r - (r * r * r - x.abs()) / (3.0 * r * r);
    if x < 0.0 { -r } else { r }
}

/// Metodos de f64 que so existem com a std.
pub trait F64Ext {
    fn fract(self) -> f64;
}

impl F64Ext for f64 {
    fn fract(self) -> f64 {
        self - trunc(self)
    }
}

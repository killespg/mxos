//! Matematica de ponto flutuante que o `core` nao tem (sem a libm): chao,
//! teto, arredondar, raiz quadrada (Newton), seno/cosseno (polinomio) e
//! potencia inteira. No PC (std) os metodos nativos tem prioridade; no
//! kernel valem estes.

pub trait F32Ext {
    fn floor(self) -> f32;
    fn ceil(self) -> f32;
    fn round(self) -> f32;
    fn sqrt(self) -> f32;
    fn sin(self) -> f32;
    fn cos(self) -> f32;
    fn powi(self, n: i32) -> f32;
    fn to_radians(self) -> f32;
}

const PI: f32 = core::f32::consts::PI;

impl F32Ext for f32 {
    fn floor(self) -> f32 {
        if !(-8.0e6..=8.0e6).contains(&self) {
            return self;
        }
        let i = self as i32 as f32;
        if i > self { i - 1.0 } else { i }
    }
    fn ceil(self) -> f32 {
        if !(-8.0e6..=8.0e6).contains(&self) {
            return self;
        }
        let i = self as i32 as f32;
        if i < self { i + 1.0 } else { i }
    }
    fn round(self) -> f32 {
        if self >= 0.0 { (self + 0.5).floor() } else { (self - 0.5).ceil() }
    }
    fn sqrt(self) -> f32 {
        if self <= 0.0 {
            return 0.0;
        }
        // estimativa pelos bits do expoente e 4 passos de Newton
        let mut x = f32::from_bits((self.to_bits() >> 1) + 0x1FC0_0000);
        for _ in 0..4 {
            x = 0.5 * (x + self / x);
        }
        x
    }
    fn sin(self) -> f32 {
        // reduz para [-pi, pi] e usa o polinomio de Taylor ate x^11
        let mut x = self % (2.0 * PI);
        if x > PI {
            x -= 2.0 * PI;
        } else if x < -PI {
            x += 2.0 * PI;
        }
        // simetria para [-pi/2, pi/2]
        if x > PI / 2.0 {
            x = PI - x;
        } else if x < -PI / 2.0 {
            x = -PI - x;
        }
        let x2 = x * x;
        x * (1.0 - x2 / 6.0 * (1.0 - x2 / 20.0 * (1.0 - x2 / 42.0 * (1.0 - x2 / 72.0 * (1.0 - x2 / 110.0)))))
    }
    fn cos(self) -> f32 {
        (self + PI / 2.0).sin()
    }
    fn powi(self, n: i32) -> f32 {
        let mut r = 1.0;
        for _ in 0..n.unsigned_abs() {
            r *= self;
        }
        if n < 0 { 1.0 / r } else { r }
    }
    fn to_radians(self) -> f32 {
        self * (PI / 180.0)
    }
}

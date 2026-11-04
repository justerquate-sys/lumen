//! Motion tokens: spring curves computed in Rust, once, at build time.
//!
//! CSS can play a spring as a `linear()` easing over a fixed duration, so
//! nothing crosses the Wasm bridge per frame: the browser's compositor runs the
//! animation and new input retargets it from its current value (CSS
//! transitions do that natively). `client.css` carries the generated block
//! verbatim; the test below fails if the two drift apart (regenerate with
//! `cargo test motion -- --nocapture` and paste the printed block).
//!
//! Model (Apple's parameters): `response` = seconds for one undamped period,
//! `damping` = damping ratio (1.0 = critically damped, no overshoot).

/// Position (0 → 1) of a unit spring released at rest, `t` seconds in.
pub fn spring_at(t: f64, response: f64, damping: f64) -> f64 {
    let w = std::f64::consts::TAU / response;
    if damping >= 1.0 {
        1.0 - (1.0 + w * t) * (-w * t).exp()
    } else {
        let wd = w * (1.0 - damping * damping).sqrt();
        let e = (-damping * w * t).exp();
        1.0 - e * ((wd * t).cos() + damping * w / wd * (wd * t).sin())
    }
}

/// Seconds until the spring stays within `eps` of its target.
pub fn settle(response: f64, damping: f64, eps: f64) -> f64 {
    let w = std::f64::consts::TAU / response;
    let mut t = 0.0;
    // Envelope bound: after this the remaining motion is below `eps` for good.
    let env = |t: f64| if damping >= 1.0 { (1.0 + w * t) * (-w * t).exp() } else { (-damping * w * t).exp() / (1.0 - damping * damping).sqrt() };
    while env(t) > eps {
        t += 0.001;
    }
    t
}

/// The spring as CSS: (`linear(...)` easing, duration in ms).
pub fn spring_css(response: f64, damping: f64, points: usize) -> (String, u32) {
    let d = settle(response, damping, 0.002);
    let mut out = String::from("linear(0");
    for i in 1..points {
        let t = d * i as f64 / points as f64;
        let v = (spring_at(t, response, damping) * 1000.0).round() / 1000.0;
        out.push_str(&format!(",{v}"));
    }
    out.push_str(",1)");
    (out, (d * 1000.0).round() as u32)
}

/// UI default spring (`--spring`, `--d-spring`): critically damped, quick.
/// Fixed samples keep curve generation and allocation off the input path;
/// the token test checks them against `spring_css`.
pub fn ui() -> (&'static str, u32) {
    ("linear(0,0.05,0.158,0.286,0.413,0.527,0.626,0.707,0.774,0.826,0.868,0.9,0.925,0.943,0.958,0.969,0.977,0.983,0.987,0.991,0.993,0.995,0.996,0.997,1)", 351)
}

/// The softer spring for big moves (`--d-spring-soft`). Critically damped
/// springs share one shape at every speed, so its curve is `--spring`'s (to rounding).
pub fn soft() -> (&'static str, u32) {
    ("linear(0,0.049,0.158,0.286,0.412,0.526,0.624,0.706,0.773,0.825,0.867,0.899,0.924,0.943,0.957,0.968,0.976,0.983,0.987,0.991,0.993,0.995,0.996,0.997,1)", 458)
}

/// A celebratory pop (`--spring-pop`): underdamped, about 10 % overshoot, for a state the user
/// just achieved by pointer (the Watch-later bookmark filling). Sits on its own line in client.css,
/// below the generated block above.
pub fn pop() -> (String, u32) {
    spring_css(0.34, 0.6, 32)
}

/// The `--spring-pop` custom properties, exactly as they appear in client.css.
pub fn css_pop_tokens() -> String {
    let (pop, ms) = pop();
    format!("--spring-pop:{pop};--d-spring-pop:{ms}ms;")
}

/// `--e-out`, for fades the bridge plays with `element.animate`.
pub const E_OUT: &str = "cubic-bezier(.23,1,.32,1)";

/// The generated custom properties, exactly as they appear in client.css.
pub fn css_tokens() -> String {
    // UI default: critically damped, quick. Momentum (flicks, drags): a little bounce.
    let (ui, ui_ms) = ui();
    let (_, soft_ms) = soft();
    let (pop, pop_ms) = spring_css(0.3, 0.82, 32);
    format!("--spring:{ui};--d-spring:{ui_ms}ms;--d-spring-soft:{soft_ms}ms;--spring-bounce:{pop};--d-spring-bounce:{pop_ms}ms;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn springs_start_at_rest_and_settle() {
        assert_eq!(spring_at(0.0, 0.3, 1.0), 0.0);
        assert!((spring_at(2.0, 0.3, 1.0) - 1.0).abs() < 1e-6);
        // Critically damped never overshoots; underdamped does, a little.
        assert!((0..400).all(|i| spring_at(i as f64 / 1000.0, 0.3, 1.0) <= 1.0));
        assert!((0..400).any(|i| spring_at(i as f64 / 1000.0, 0.3, 0.82) > 1.0));
        let (curve, ms) = spring_css(0.26, 1.0, 24);
        assert!(curve.starts_with("linear(0,") && curve.ends_with(",1)"));
        assert!((250..450).contains(&ms), "{ms}");
    }

    #[test]
    fn client_css_carries_the_generated_tokens() {
        for (actual, response) in [(ui(), 0.26), (soft(), 0.34)] {
            let expected = spring_css(response, 1.0, 24);
            assert_eq!((actual.0, actual.1), (expected.0.as_str(), expected.1));
        }
        let tokens = css_tokens();
        println!("{tokens}");
        let css = include_str!("../extension/client.css");
        assert!(css.contains(&tokens), "client.css :root motion block is stale; paste:\n{tokens}");
        let pop = css_pop_tokens();
        println!("{pop}");
        assert!(css.contains(&pop), "client.css --spring-pop line is stale; paste:\n{pop}");
        assert!(css.contains(&format!("--e-out:{E_OUT};")), "motion::E_OUT differs from client.css --e-out");
        // The soft spring is --spring's shape over a longer time (sampling differs by rounding only).
        let parse = |c: &str| c.trim_start_matches("linear(").trim_end_matches(')').split(',').map(|x| x.parse::<f64>().unwrap()).collect::<Vec<_>>();
        let (a, b) = (parse(&ui().0), parse(&soft().0));
        assert!(a.len() == b.len() && a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 0.003), "{a:?} vs {b:?}");
    }
}

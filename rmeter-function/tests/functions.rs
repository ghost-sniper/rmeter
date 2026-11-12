use rmeter_function::*;
use std::collections::HashMap;

struct Fixed(u64);
impl Source for Fixed {
    fn now_millis(&mut self) -> i64 {
        0
    }
    fn random_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1);
        self.0
    }
}
fn environment() -> Environment {
    Environment {
        timezone: "UTC".into(),
        locale: "en-US".into(),
    }
}
#[test]
fn functions_ranges_unicode_and_errors() {
    let mut rng = Fixed(123);
    for _ in 0..1000 {
        assert!((-2..=2).contains(&random(-2, 2, &mut rng).unwrap()));
        random(i64::MIN, i64::MAX, &mut rng).unwrap();
    }
    assert_eq!(random(i64::MAX, i64::MAX, &mut rng).unwrap(), i64::MAX);
    assert!(random(3, 2, &mut rng).is_err());
    for chars in [None, Some("a中😀"), Some("😀")] {
        let s = random_string(100, chars, &mut rng).unwrap();
        assert_eq!(s.encode_utf16().count(), 100);
    }
    assert!(random_string(1, Some("😀"), &mut rng).is_err());
    assert!(random_string(MAX_RANDOM_LENGTH + 1, None, &mut rng).is_err());
    assert_eq!(change_case("Straße", "upper").unwrap(), "STRASSE");
    assert_eq!(
        change_case("hELLO world", "CAPITALIZE").unwrap(),
        "HELLO world"
    );
    assert!(change_case("x", "bad").is_err());
}
#[test]
fn expressions_nesting_order_escaping_and_rollback() {
    let mut variables = HashMap::from([("day".into(), "2024-02-29".into())]);
    let mut rng = Fixed(1);
    let env = environment();
    let mut ctx = FunctionContext {
        variables: &mut variables,
        environment: &env,
        source: &mut rng,
    };
    let expr=Expression::compile("${__changeCase(${__dateTimeConvert(${day},yyyy-MM-dd,dd/MM/yyyy,saved)},UPPER,result)}-${saved}-${result}").unwrap();
    assert_eq!(
        expr.evaluate(&mut ctx).unwrap(),
        "29/02/2024-29/02/2024-29/02/2024"
    );
    assert_eq!(
        Expression::compile(r"${__changeCase(a\,b\)c,UPPER)}")
            .unwrap()
            .evaluate(&mut ctx)
            .unwrap(),
        "A,B)C"
    );
    let before = ctx.variables.clone();
    assert!(
        Expression::compile("${__Random(1,1,write)}${missing}")
            .unwrap()
            .evaluate(&mut ctx)
            .is_err()
    );
    assert_eq!(*ctx.variables, before);
    assert!(Expression::compile("${__Unknown()}").is_err());
    assert!(Expression::compile("${__Random(1)}").is_err());
    ctx.variables
        .insert("literal".into(), "${__Random(1,1)}".into());
    assert_eq!(
        Expression::compile("${literal}")
            .unwrap()
            .evaluate(&mut ctx)
            .unwrap(),
        "${__Random(1,1)}"
    );
    let mut nested = "x".to_string();
    for _ in 0..40 {
        nested = format!("${{__changeCase({nested})}}");
    }
    assert!(Expression::compile(&nested).is_err());
}
#[test]
fn random_dates_use_exclusive_end_and_injected_clock() {
    let env = environment();
    let mut source = Fixed(5);
    for _ in 0..100 {
        assert_eq!(
            date::random_date("", "2024-02-29", "2024-03-01", "", &env, &mut source).unwrap(),
            "2024-02-29"
        );
    }
    assert_eq!(
        date::random_date("", "", "1970-01-02", "", &env, &mut source).unwrap(),
        "1970-01-01"
    );
    assert!(date::random_date("", "2024-01-01", "2024-01-01", "", &env, &mut source).is_err());
}
#[test]
fn java21_differential_fixtures() {
    #[derive(serde::Deserialize, Debug)]
    struct Case {
        input: String,
        source: String,
        target: String,
        locale: String,
        zone: String,
        expected: Option<String>,
    }
    let mut failures = Vec::new();
    let mut count = 0;
    let mut seen = std::collections::HashSet::new();
    let bytes = flate2::read::GzDecoder::new(&include_bytes!("fixtures/java21.jsonl.gz")[..]);
    use std::io::BufRead;
    for line in std::io::BufReader::new(bytes).lines() {
        let line = line.unwrap();
        let case: Case = serde_json::from_str(&line).unwrap();
        let actual = date::date_time_convert(
            &case.input,
            &case.source,
            &case.target,
            &Environment {
                timezone: case.zone.clone(),
                locale: case.locale.clone(),
            },
        );
        if actual.as_ref().ok() != case.expected.as_ref() {
            count += 1;
            if failures.len() < 35 && seen.insert((case.source.clone(), case.target.clone())) {
                failures.push(format!("{case:?} => {actual:?}"));
            }
        }
    }
    assert_eq!(count, 0, "{}", failures.join("\n"));
}

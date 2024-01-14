const BASE36_DIGITS: [char; 36] = ['0','1','2','3','4','5','6','7','8','9','a','b','c','d','e','f','g','h','i','j','k','l','m','n','o','p','q','r','s','t','u','v','w','x','y','z'];

pub fn lossy_u64_from_base36(input: &str) -> u64 {
    input.chars().filter_map(|c| BASE36_DIGITS.iter().position(|&x| x == c)).rev().enumerate().map(|(index, digit)| (digit as u64) * (36u64.pow(index as u32))).sum()
}

pub fn base36_from_i64(mut input: i64) -> String {
    let mut chars = Vec::new();
    if input == 0 {
        return "0".to_string()
    }
    while input > 0 {
        chars.push(BASE36_DIGITS[(input % 36) as usize]);
        input /= 36;
    }
    String::from_iter(chars.iter().rev())
}

pub fn base36_to_base10(input: i8) -> i32 {
    let mut result = 0;
    let mut base = 1;
    let mut num = input.abs() as i32;

    while num > 0 {
        let digit = num % 10;
        result += digit * base;
        num /= 10;
        base *= 36;
    }

    result * if input.is_negative() { -1 } else { 1 }
}

pub fn look_to_i8_range(yaw: f32, pitch: f32) -> (i8, i8) {
    let yawf = ((yaw / 360.) * 255.) % 255.;
    let pitch = (((pitch / 360.) * 255.) % 255.) as i8;

    let mut yaw = yawf as i8;
    if yawf < -128. {
        yaw = 127 - (yawf + 128.).abs() as i8
    }
    if yawf > 128. {
        yaw = -128 + (yawf - 128.).abs() as i8
    }
    (yaw, pitch)
}

#[test]
fn test_base_conv() {
    assert_eq!(base36_to_base10(18), 44);
    println!("base: {}", base36_to_base10(-127));
    println!("{}", (12 << 4));
}

#[test]
fn test_lossy_u64_from_base36() {
    assert_eq!(lossy_u64_from_base36("ya"), 1234);
    assert_eq!(lossy_u64_from_base36("7cik2"), 12341234);
}

#[test]
fn test_base36_from_u64() {
    assert_eq!(base36_from_i64(1234), "ya");
    assert_eq!(base36_from_i64(12341234), "7cik2");
}

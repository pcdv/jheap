//! How numbers and names are written, and coloured on a terminal.

/// Colour, or none: in a pipe, a file or when NO_COLOR is set, text stays plain.
#[derive(Clone, Copy)]
pub struct Style {
    pub color: bool,
}

impl Style {
    fn paint(&self, code: &str, s: &str) -> String {
        if self.color && !s.is_empty() { format!("\x1b[{code}m{s}\x1b[0m") } else { s.to_string() }
    }

    pub fn bold(&self, s: &str) -> String {
        self.paint("1", s)
    }

    pub fn dim(&self, s: &str) -> String {
        self.paint("2", s)
    }

    pub fn warn(&self, s: &str) -> String {
        self.paint("1;33", s)
    }

    pub fn good(&self, s: &str) -> String {
        self.paint("32", s)
    }
}

/// Bytes in the unit that keeps them short: 512 B, 1.5 KiB, 3.2 GiB.
pub fn bytes(n: f64) -> String {
    const K: f64 = 1024.0;
    if n < K {
        format!("{} B", n.round())
    } else if n < K * K {
        format!("{:.1} KiB", n / K)
    } else if n < K * K * K {
        format!("{:.1} MiB", n / K / K)
    } else {
        format!("{:.1} GiB", n / K / K / K)
    }
}

/// A share, 0 to 1, as a percent with the digits it needs: 85%, 2.5%, <0.1%.
pub fn percent(share: f64) -> String {
    let p = share * 100.0;
    if p >= 10.0 || p == 0.0 {
        format!("{p:.0}%")
    } else if p >= 0.1 {
        format!("{p:.1}%")
    } else {
        "<0.1%".into()
    }
}

/// A count with its thousands apart: 1,399,686.
pub fn count(n: f64) -> String {
    let digits = format!("{}", n.round().abs() as u64);
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0.0 { format!("-{out}") } else { out }
}

/// `n things`, singular for one.
pub fn counted(n: f64, thing: &str) -> String {
    format!("{} {thing}{}", count(n), if n == 1.0 { "" } else { "s" })
}

pub fn duration(ms: f64) -> String {
    if ms >= 10_000.0 {
        format!("{:.1} s", ms / 1000.0)
    } else if ms >= 1000.0 {
        format!("{:.2} s", ms / 1000.0)
    } else if ms >= 100.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{ms:.1} ms")
    }
}

/// A class name without its package: what is enough where the full name is
/// on the line above.
pub fn simple(name: &str) -> &str {
    &name[name.rfind('.').map_or(0, |i| i + 1)..]
}

/// What a root says of how long what it holds lives.
pub fn lives(root: &str) -> Option<&'static str> {
    Some(match root {
        "System Class" => "from a static field: it lives as long as its class",
        "JNI Global" => "from a JNI global reference, a class loader most often: as long as it is loaded",
        "Java Frame" | "JNI Local" | "Native Stack" | "Thread Block" | "Monitor" => {
            "from a thread's stack: as long as the thread holds it"
        }
        "Thread" => "from a live thread: as long as it runs",
        "(unreachable)" => "nothing reaches it: garbage the next collection frees",
        _ => return None,
    })
}

/// The same, in a few words.
pub fn held_from(root: &str) -> &'static str {
    match root {
        "System Class" => "from a static field",
        "JNI Global" => "from a JNI global reference",
        "Java Frame" | "JNI Local" | "Native Stack" | "Thread Block" | "Monitor" => "from a thread's stack",
        "Thread" => "from a live thread",
        "(unreachable)" => "nothing reaches it: garbage",
        "Unknown" | "GC Root" => "from a root of unknown kind",
        // a path deeper than the frames kept starts with a class, its root cut off
        _ => "from a root past the 31 frames kept",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(bytes(512.0), "512 B");
        assert_eq!(bytes(1536.0), "1.5 KiB");
        assert_eq!(bytes(3.0 * 1024.0 * 1024.0 * 1024.0), "3.0 GiB");
        assert_eq!(percent(0.8496), "85%");
        assert_eq!(percent(0.025), "2.5%");
        assert_eq!(percent(0.0001), "<0.1%");
        assert_eq!(percent(0.0), "0%");
        assert_eq!(count(1399686.0), "1,399,686");
        assert_eq!(count(-700.0), "-700");
        assert_eq!(counted(1.0, "object"), "1 object");
        assert_eq!(simple("com.example.Registry"), "Registry");
    }
}

//! HPROF's record tags, heap dump sub-record tags and value types.
//!
//! Reference: <https://github.com/openjdk/jdk/blob/master/src/hotspot/share/services/heapDumper.cpp>

// Top-level records
pub(crate) const STRING_IN_UTF8: u8 = 0x01;
pub(crate) const LOAD_CLASS: u8 = 0x02;
pub(crate) const UNLOAD_CLASS: u8 = 0x03;
pub(crate) const STACK_FRAME: u8 = 0x04;
pub(crate) const STACK_TRACE: u8 = 0x05;
pub(crate) const ALLOC_SITES: u8 = 0x06;
pub(crate) const HEAP_SUMMARY: u8 = 0x07;
pub(crate) const START_THREAD: u8 = 0x0a;
pub(crate) const END_THREAD: u8 = 0x0b;
pub(crate) const HEAP_DUMP: u8 = 0x0c;
pub(crate) const CPU_SAMPLES: u8 = 0x0d;
pub(crate) const CONTROL_SETTINGS: u8 = 0x0e;
pub(crate) const HEAP_DUMP_SEGMENT: u8 = 0x1c;
pub(crate) const HEAP_DUMP_END: u8 = 0x2c;

/// The top-level records an HPROF file can have: any other tag is a corrupt file.
pub(crate) const RECORD_TAGS: [u8; 14] = [
    STRING_IN_UTF8, LOAD_CLASS, UNLOAD_CLASS, STACK_FRAME, STACK_TRACE, ALLOC_SITES,
    HEAP_SUMMARY, START_THREAD, END_THREAD, HEAP_DUMP, CPU_SAMPLES, CONTROL_SETTINGS,
    HEAP_DUMP_SEGMENT, HEAP_DUMP_END,
];

// Sub-records of a heap dump or one of its segments
pub(crate) const ROOT_UNKNOWN: u8 = 0xff;
pub(crate) const ROOT_JNI_GLOBAL: u8 = 0x01;
pub(crate) const ROOT_JNI_LOCAL: u8 = 0x02;
pub(crate) const ROOT_JAVA_FRAME: u8 = 0x03;
pub(crate) const ROOT_NATIVE_STACK: u8 = 0x04;
pub(crate) const ROOT_STICKY_CLASS: u8 = 0x05;
pub(crate) const ROOT_THREAD_BLOCK: u8 = 0x06;
pub(crate) const ROOT_MONITOR_USED: u8 = 0x07;
pub(crate) const ROOT_THREAD_OBJECT: u8 = 0x08;
pub(crate) const CLASS_DUMP: u8 = 0x20;
pub(crate) const INSTANCE_DUMP: u8 = 0x21;
pub(crate) const OBJECT_ARRAY_DUMP: u8 = 0x22;
pub(crate) const PRIMITIVE_ARRAY_DUMP: u8 = 0x23;

// Types of fields and array elements
pub(crate) const TYPE_OBJECT: u8 = 2;
pub(crate) const TYPE_BOOLEAN: u8 = 4;
pub(crate) const TYPE_CHAR: u8 = 5;
pub(crate) const TYPE_FLOAT: u8 = 6;
pub(crate) const TYPE_DOUBLE: u8 = 7;
pub(crate) const TYPE_BYTE: u8 = 8;
pub(crate) const TYPE_SHORT: u8 = 9;
pub(crate) const TYPE_INT: u8 = 10;
pub(crate) const TYPE_LONG: u8 = 11;

/// The bytes of a primitive value; 0 for a reference, whose width is the
/// dump's ids, and for a type the format does not have.
#[inline]
pub(crate) fn type_size(t: u8) -> u64 {
    match t {
        TYPE_BOOLEAN | TYPE_BYTE => 1,
        TYPE_CHAR | TYPE_SHORT => 2,
        TYPE_FLOAT | TYPE_INT => 4,
        TYPE_DOUBLE | TYPE_LONG => 8,
        _ => 0,
    }
}

/// How a root is named in a path: its first frame.
pub(crate) fn root_name(t: u8) -> Option<&'static str> {
    Some(match t {
        ROOT_UNKNOWN => "Unknown",
        ROOT_JNI_GLOBAL => "JNI Global",
        ROOT_JNI_LOCAL => "JNI Local",
        ROOT_JAVA_FRAME => "Java Frame",
        ROOT_NATIVE_STACK => "Native Stack",
        ROOT_STICKY_CLASS => "System Class",
        ROOT_THREAD_BLOCK => "Thread Block",
        ROOT_MONITOR_USED => "Monitor",
        ROOT_THREAD_OBJECT => "Thread",
        _ => return None,
    })
}

/// The bytes a root record has after its object id, or None for a tag that is
/// not a root's.
#[inline]
pub(crate) fn root_tail(t: u8, id_size: usize) -> Option<u64> {
    match t {
        ROOT_UNKNOWN | ROOT_STICKY_CLASS | ROOT_MONITOR_USED => Some(0),
        // the JNI reference
        ROOT_JNI_GLOBAL => Some(id_size as u64),
        // thread serial
        ROOT_NATIVE_STACK | ROOT_THREAD_BLOCK => Some(4),
        // thread serial and frame number, or thread serial and stack serial
        ROOT_JNI_LOCAL | ROOT_JAVA_FRAME | ROOT_THREAD_OBJECT => Some(8),
        _ => None,
    }
}

/// The class name of a primitive array.
pub(crate) fn primitive_array_name(t: u8) -> &'static str {
    match t {
        TYPE_BOOLEAN => "boolean[]",
        TYPE_CHAR => "char[]",
        TYPE_FLOAT => "float[]",
        TYPE_DOUBLE => "double[]",
        TYPE_BYTE => "byte[]",
        TYPE_SHORT => "short[]",
        TYPE_INT => "int[]",
        TYPE_LONG => "long[]",
        _ => "primitive[]",
    }
}

/// A class name as the JVM writes it, made readable: `Ljava/lang/String;` is
/// `java.lang.String`, `[I` is `int[]`.
pub(crate) fn format_class_name(name: &str) -> String {
    if name.starts_with('[') {
        let depth = name.bytes().take_while(|&b| b == b'[').count();
        let elem = &name[depth..];
        let base = match elem {
            "Z" => "boolean".to_string(),
            "B" => "byte".to_string(),
            "C" => "char".to_string(),
            "S" => "short".to_string(),
            "I" => "int".to_string(),
            "J" => "long".to_string(),
            "F" => "float".to_string(),
            "D" => "double".to_string(),
            _ if elem.len() >= 2 && elem.starts_with('L') && elem.ends_with(';') => {
                elem[1..elem.len() - 1].replace('/', ".")
            }
            _ => elem.to_string(),
        };
        return base + &"[]".repeat(depth);
    }
    if name.len() >= 2 && name.starts_with('L') && name.ends_with(';') {
        return name[1..name.len() - 1].replace('/', ".");
    }
    name.replace('/', ".")
}

#[cfg(test)]
mod tests {
    use super::format_class_name;

    #[test]
    fn class_names() {
        assert_eq!(format_class_name("java/lang/String"), "java.lang.String");
        assert_eq!(format_class_name("Ljava/lang/String;"), "java.lang.String");
        assert_eq!(format_class_name("[I"), "int[]");
        assert_eq!(format_class_name("[[Ljava/util/HashMap$Node;"), "java.util.HashMap$Node[][]");
        assert_eq!(format_class_name("[Q"), "Q[]");
    }
}

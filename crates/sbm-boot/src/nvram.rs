use alloc::string::String;

use crate::pathutil::normalize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedLoadOption {
    pub attributes: u32,
    pub description: String,
    pub file_path: Option<String>,
    pub partition_guid: Option<String>,
}

pub fn parse_load_option(bytes: &[u8]) -> Option<ParsedLoadOption> {
    if bytes.len() < 6 {
        return None;
    }
    let attributes = u32::from_le_bytes(bytes[0..4].try_into().ok()?);
    let list_len = u16::from_le_bytes(bytes[4..6].try_into().ok()?) as usize;
    let mut index = 6;
    let mut description = String::new();
    loop {
        if index + 2 > bytes.len() {
            return None;
        }
        let unit = u16::from_le_bytes(bytes[index..index + 2].try_into().ok()?);
        index += 2;
        if unit == 0 {
            break;
        }
        description.push(char::from_u32(u32::from(unit)).unwrap_or('?'));
    }
    let list_end = index.checked_add(list_len)?;
    if list_end > bytes.len() {
        return None;
    }
    let (file_path, partition_guid) = parse_nodes(&bytes[index..list_end]);
    Some(ParsedLoadOption {
        attributes,
        description,
        file_path,
        partition_guid,
    })
}

fn parse_nodes(mut nodes: &[u8]) -> (Option<String>, Option<String>) {
    let mut file_path = None;
    let mut partition_guid = None;
    while nodes.len() >= 4 {
        let kind = nodes[0];
        let sub = nodes[1];
        let len = u16::from_le_bytes([nodes[2], nodes[3]]) as usize;
        if len < 4 || len > nodes.len() {
            break;
        }
        let body = &nodes[4..len];
        if kind == 0x7f {
            break;
        }
        if kind == 0x04 && sub == 0x01 && body.len() >= 38 && body[37] == 2 {
            partition_guid = Some(format_guid(&body[20..36]));
        }
        if kind == 0x04 && sub == 0x04 {
            file_path = Some(normalize(&utf16_lossy(body)));
        }
        nodes = &nodes[len..];
    }
    (file_path, partition_guid)
}

fn format_guid(bytes: &[u8]) -> String {
    let data1 = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
    let data2 = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
    let data3 = u16::from_le_bytes(bytes[6..8].try_into().unwrap());
    alloc::format!(
        "{data1:08x}-{data2:04x}-{data3:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

fn utf16_lossy(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut index = 0;
    while index + 1 < bytes.len() {
        let unit = u16::from_le_bytes([bytes[index], bytes[index + 1]]);
        index += 2;
        if unit == 0 {
            break;
        }
        out.push(char::from_u32(u32::from(unit)).unwrap_or('?'));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(text: &str) -> alloc::vec::Vec<u8> {
        let mut out = alloc::vec::Vec::new();
        for ch in text.encode_utf16() {
            out.extend_from_slice(&ch.to_le_bytes());
        }
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    #[test]
    fn parses_description_path_and_gpt_signature() {
        let desc = utf16("Ubuntu");
        let path = utf16(r"\EFI\ubuntu\shimx64.efi");
        let mut guid = [0u8; 16];
        guid[0..4].copy_from_slice(&0x0011_2233u32.to_le_bytes());
        guid[4..6].copy_from_slice(&0x4455u16.to_le_bytes());
        guid[6..8].copy_from_slice(&0x6677u16.to_le_bytes());
        guid[8..].copy_from_slice(&[0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);

        let mut hd = alloc::vec![0u8; 42];
        hd[0] = 0x04;
        hd[1] = 0x01;
        hd[2] = 42;
        hd[24..40].copy_from_slice(&guid);
        hd[41] = 2;

        let mut file = alloc::vec![0x04, 0x04, 0, 0];
        let file_len = (4 + path.len()) as u16;
        file[2..4].copy_from_slice(&file_len.to_le_bytes());
        file.extend_from_slice(&path);
        let end = [0x7fu8, 0xff, 4, 0];

        let mut list = hd.clone();
        list.extend_from_slice(&file);
        list.extend_from_slice(&end);

        let mut bytes = alloc::vec::Vec::new();
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&(list.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&desc);
        bytes.extend_from_slice(&list);

        let parsed = parse_load_option(&bytes).unwrap();
        assert_eq!(parsed.description, "Ubuntu");
        assert_eq!(
            parsed.file_path.as_deref(),
            Some(r"\EFI\ubuntu\shimx64.efi")
        );
        assert_eq!(
            parsed.partition_guid.as_deref(),
            Some("00112233-4455-6677-8899-aabbccddeeff")
        );
    }
}

use std::io::BufReader;

use mcpeval::frame::{read_frame, Frame};

#[test]
fn reads_two_messages_and_preserves_bytes() {
    let input = b"{\"jsonrpc\":\"2.0\",\"id\":1}\n{\"jsonrpc\":\"2.0\",\"id\":2}\n";
    let mut r = BufReader::new(&input[..]);

    let first: Frame = read_frame(&mut r).unwrap().unwrap();
    assert_eq!(first.raw, b"{\"jsonrpc\":\"2.0\",\"id\":1}\n");
    assert_eq!(first.value.unwrap()["id"], 1);

    let second = read_frame(&mut r).unwrap().unwrap();
    assert_eq!(second.value.unwrap()["id"], 2);

    assert!(
        read_frame(&mut r).unwrap().is_none(),
        "third read must be EOF"
    );
}

#[test]
fn unparsable_line_is_still_a_frame() {
    let input = b"this is not json\n";
    let mut r = BufReader::new(&input[..]);
    let f = read_frame(&mut r).unwrap().unwrap();
    assert_eq!(f.raw, b"this is not json\n");
    assert!(f.value.is_none());
}

#[test]
fn final_line_without_newline_is_returned() {
    let input = b"{\"id\":9}";
    let mut r = BufReader::new(&input[..]);
    let f = read_frame(&mut r).unwrap().unwrap();
    assert_eq!(f.raw, b"{\"id\":9}");
    assert_eq!(f.value.unwrap()["id"], 9);
}

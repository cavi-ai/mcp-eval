use std::io::BufReader;

use mcpeval::frame::{read_frame, Frame};

#[test]
fn reads_two_messages_and_preserves_bytes() {
    let input = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{}}\n";
    let mut r = BufReader::new(&input[..]);

    let first: Frame = read_frame(&mut r).unwrap().unwrap();
    assert_eq!(first.raw, b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n");
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
    let input = b"{\"jsonrpc\":\"2.0\",\"id\":9,\"result\":{}}";
    let mut r = BufReader::new(&input[..]);
    let f = read_frame(&mut r).unwrap().unwrap();
    assert_eq!(f.raw, input);
    assert_eq!(f.value.unwrap()["id"], 9);
}

#[test]
fn semantically_invalid_json_rpc_is_unparsed_but_bytes_are_preserved() {
    for input in [
        b"[]\n".as_slice(),
        b"{}\n".as_slice(),
        b"{\"jsonrpc\":\"1.0\",\"id\":1,\"method\":\"ping\"}\n".as_slice(),
        b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{},\"error\":{\"code\":-1,\"message\":\"x\"}}\n"
            .as_slice(),
        b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"bad path?secret=x\"}\n".as_slice(),
    ] {
        let mut reader = BufReader::new(input);
        let frame = read_frame(&mut reader).unwrap().unwrap();
        assert_eq!(frame.raw, input);
        assert!(frame.value.is_none(), "accepted invalid frame: {input:?}");
    }
}

#[test]
fn accepts_requests_notifications_and_responses_in_both_directions() {
    for input in [
        br#"{"jsonrpc":"2.0","id":"x","method":"sampling/createMessage","params":{}}"#.as_slice(),
        br#"{"jsonrpc":"2.0","method":"notifications/message","params":[]}"#.as_slice(),
        br#"{"jsonrpc":"2.0","id":"x","result":{}}"#.as_slice(),
        br#"{"jsonrpc":"2.0","id":2,"error":{"code":-32600,"message":"bad"}}"#.as_slice(),
    ] {
        let mut reader = BufReader::new(input);
        assert!(read_frame(&mut reader).unwrap().unwrap().value.is_some());
    }
}

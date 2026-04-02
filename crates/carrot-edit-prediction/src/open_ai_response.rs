pub fn text_from_response(mut res: carrot_open_ai::Response) -> Option<String> {
    let choice = res.choices.pop()?;
    let output_text = match choice.message {
        carrot_open_ai::RequestMessage::Assistant {
            content: Some(carrot_open_ai::MessageContent::Plain(content)),
            ..
        } => content,
        carrot_open_ai::RequestMessage::Assistant {
            content: Some(carrot_open_ai::MessageContent::Multipart(mut content)),
            ..
        } => {
            if content.is_empty() {
                log::error!("No output from Baseten completion response");
                return None;
            }

            match content.remove(0) {
                carrot_open_ai::MessagePart::Text { text } => text,
                carrot_open_ai::MessagePart::Image { .. } => {
                    log::error!("Expected text, got an image");
                    return None;
                }
            }
        }
        _ => {
            log::error!("Invalid response message: {:?}", choice.message);
            return None;
        }
    };
    Some(output_text)
}

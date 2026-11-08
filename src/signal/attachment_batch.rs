use presage::libsignal_service::sender::AttachmentSpec;

type Attachment = (AttachmentSpec, Vec<u8>);

/// Keep contiguous images supported by Signal Desktop together; send other files
/// individually so a generic attachment cannot hide subsequent album entries.
/// Empty attachments retain one batch so a text-only message is sent once.
pub(crate) fn compatible_batches(attachments: Vec<Attachment>) -> Vec<Vec<Attachment>> {
    if attachments.is_empty() {
        return vec![Vec::new()];
    }
    let mut batches = Vec::new();
    let mut album = Vec::new();
    for attachment in attachments {
        // Match the exact canonical MIME values recognized by official clients.
        // A noncanonical/unknown value remains a standalone file.
        let compatible = attachment.0.voice_note != Some(true)
            && matches!(
                attachment.0.content_type.as_str(),
                "image/jpeg" | "image/png" | "image/webp"
            );
        if compatible {
            album.push(attachment);
        } else {
            if !album.is_empty() {
                batches.push(std::mem::take(&mut album));
            }
            batches.push(vec![attachment]);
        }
    }
    if !album.is_empty() {
        batches.push(album);
    }
    batches
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(mime: &str, index: u8) -> (AttachmentSpec, Vec<u8>) {
        (
            AttachmentSpec {
                content_type: mime.into(),
                file_name: Some(format!("file-{index}")),
                length: 2,
                ..Default::default()
            },
            vec![index, index + 1],
        )
    }

    #[test]
    fn unsupported_file_splits_album_without_reordering_or_changing_payloads() {
        let types = [
            "image/jpeg",
            "image/jpeg",
            "image/jpeg",
            "application/octet-stream",
            "image/jpeg",
            "image/png",
        ];
        let batches = compatible_batches(
            types
                .iter()
                .enumerate()
                .map(|(i, mime)| file(mime, i as u8))
                .collect(),
        );
        assert_eq!(batches.iter().map(Vec::len).collect::<Vec<_>>(), [3, 1, 2]);
        for (index, (spec, bytes)) in batches.into_iter().flatten().enumerate() {
            assert_eq!(spec.content_type, types[index]);
            assert_eq!(spec.file_name, Some(format!("file-{index}")));
            assert_eq!(spec.length, 2);
            assert_eq!(bytes, [index as u8, index as u8 + 1]);
        }
    }

    #[test]
    fn documents_heic_gif_audio_and_video_each_send_individually() {
        let types = [
            "application/pdf",
            "application/pdf",
            "image/heic",
            "image/gif",
            "audio/ogg",
            "video/mp4",
        ];
        let batches = compatible_batches(
            types
                .iter()
                .enumerate()
                .map(|(i, mime)| file(mime, i as u8))
                .collect(),
        );
        assert_eq!(batches.len(), types.len());
        assert!(batches.iter().all(|batch| batch.len() == 1));
    }

    #[test]
    fn noncanonical_mime_and_voice_note_each_stay_standalone() {
        let first = file(" IMAGE/JPEG ; charset=binary", 0);
        let second = file("image/WebP", 1);
        let mut voice = file("image/png", 2);
        voice.0.voice_note = Some(true);
        let batches = compatible_batches(vec![first, second, voice, file("image/png", 3)]);
        assert_eq!(
            batches.iter().map(Vec::len).collect::<Vec<_>>(),
            [1, 1, 1, 1]
        );
        assert_eq!(batches[2][0].0.voice_note, Some(true));
    }

    #[test]
    fn empty_payload_still_produces_one_text_send() {
        let batches = compatible_batches(Vec::new());
        assert_eq!(batches.len(), 1);
        assert!(batches[0].is_empty());
    }
}

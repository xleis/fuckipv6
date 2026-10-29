use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use fuckipv6_secure_session::{
    Direction, EpochSession, ReceiveCipher, SendCipher, SessionError, AEAD_TAG_LEN,
    DATA_HEADER_LEN, SESSION_ID_LEN,
};

const KEY: [u8; 32] = [0x5a; 32];
const SESSION_ID: [u8; SESSION_ID_LEN] = [0x33; SESSION_ID_LEN];
const LOCAL_RECEIVER_INDEX: u32 = 0x1122_3344;

fn bench_packet_paths(c: &mut Criterion) {
    for payload_len in [64usize, 512, 1280, 1420] {
        let plaintext = vec![0x42; payload_len];
        let packet_capacity = DATA_HEADER_LEN + payload_len + AEAD_TAG_LEN;
        let mut group = c.benchmark_group(format!("secure_session_v2_{payload_len}"));
        group.throughput(Throughput::Bytes(payload_len as u64));

        group.bench_function("seal", |b| {
            let mut sender = SendCipher::new(
                KEY,
                SESSION_ID,
                Direction::InitiatorToResponder,
                1,
                LOCAL_RECEIVER_INDEX,
            );
            let mut packet = vec![0; packet_capacity];
            b.iter(|| {
                sender
                    .seal(black_box(&plaintext), black_box(&mut packet))
                    .expect("seal benchmark packet")
            })
        });

        let mut sender = SendCipher::new(
            KEY,
            SESSION_ID,
            Direction::InitiatorToResponder,
            1,
            LOCAL_RECEIVER_INDEX,
        );
        let mut sealed = vec![0; packet_capacity];
        let sealed_len = sender
            .seal(&plaintext, &mut sealed)
            .expect("seal fixture packet");
        let sealed = sealed[..sealed_len].to_vec();

        group.bench_function("open", |b| {
            b.iter_batched(
                || {
                    (
                        ReceiveCipher::new(
                            KEY,
                            SESSION_ID,
                            Direction::InitiatorToResponder,
                            1,
                            LOCAL_RECEIVER_INDEX,
                        ),
                        sealed.clone(),
                    )
                },
                |(mut receiver, mut packet)| {
                    receiver
                        .open_in_place(black_box(&mut packet))
                        .expect("open benchmark packet")
                        .len()
                },
                BatchSize::SmallInput,
            )
        });

        let mut replay_receiver = ReceiveCipher::new(
            KEY,
            SESSION_ID,
            Direction::InitiatorToResponder,
            1,
            LOCAL_RECEIVER_INDEX,
        );
        let mut replay_packet = sealed.clone();
        replay_receiver
            .open_in_place(&mut replay_packet)
            .expect("prime replay fixture");
        group.bench_function("replay_reject", |b| {
            b.iter(|| {
                let result = replay_receiver.open_in_place(black_box(&mut replay_packet));
                debug_assert_eq!(result, Err(SessionError::ReplayDetected));
            })
        });

        group.finish();
    }
}

fn bench_batch_paths(c: &mut Criterion) {
    const BATCH_PACKETS: usize = 32;

    for payload_len in [1280usize, 1420] {
        let plaintext = vec![0x42; payload_len];
        let plaintexts = vec![plaintext.as_slice(); BATCH_PACKETS];
        let stride = DATA_HEADER_LEN + payload_len + AEAD_TAG_LEN;
        let batch_bytes = stride * BATCH_PACKETS;
        let mut group = c.benchmark_group(format!("secure_session_v2_batch_{payload_len}"));
        group.throughput(Throughput::Bytes((payload_len * BATCH_PACKETS) as u64));

        group.bench_function("seal_single_x32", |b| {
            let mut sender = SendCipher::new(
                KEY,
                SESSION_ID,
                Direction::InitiatorToResponder,
                1,
                LOCAL_RECEIVER_INDEX,
            );
            let mut packets = vec![0u8; batch_bytes];
            b.iter(|| {
                for (index, plaintext) in plaintexts.iter().enumerate() {
                    let offset = index * stride;
                    sender
                        .seal(
                            black_box(*plaintext),
                            black_box(&mut packets[offset..offset + stride]),
                        )
                        .expect("single seal batch benchmark packet");
                }
                black_box(packets.len())
            })
        });

        group.bench_function("seal_batch_x32", |b| {
            let mut sender = SendCipher::new(
                KEY,
                SESSION_ID,
                Direction::InitiatorToResponder,
                1,
                LOCAL_RECEIVER_INDEX,
            );
            let mut packets = vec![0u8; batch_bytes];
            b.iter(|| {
                sender
                    .seal_batch(
                        black_box(&plaintexts),
                        black_box(&mut packets),
                        black_box(stride),
                    )
                    .expect("batch seal benchmark packets")
            })
        });

        let mut sealing = SendCipher::new(
            KEY,
            SESSION_ID,
            Direction::InitiatorToResponder,
            1,
            LOCAL_RECEIVER_INDEX,
        );
        let mut sealed = vec![0u8; batch_bytes];
        let mut packet_lengths = Vec::with_capacity(BATCH_PACKETS);
        for (index, plaintext) in plaintexts.iter().enumerate() {
            let offset = index * stride;
            packet_lengths.push(
                sealing
                    .seal(plaintext, &mut sealed[offset..offset + stride])
                    .expect("seal open-batch fixture"),
            );
        }

        group.bench_function("open_single_x32", |b| {
            b.iter_batched(
                || {
                    (
                        ReceiveCipher::new(
                            KEY,
                            SESSION_ID,
                            Direction::InitiatorToResponder,
                            1,
                            LOCAL_RECEIVER_INDEX,
                        ),
                        sealed.clone(),
                    )
                },
                |(mut receiver, mut packets)| {
                    let mut plaintext_bytes = 0;
                    for (index, packet_len) in packet_lengths.iter().copied().enumerate() {
                        let offset = index * stride;
                        plaintext_bytes += receiver
                            .open_in_place(black_box(&mut packets[offset..offset + packet_len]))
                            .expect("single open batch benchmark packet")
                            .len();
                    }
                    black_box(plaintext_bytes)
                },
                BatchSize::PerIteration,
            )
        });

        group.bench_function("open_batch_x32", |b| {
            b.iter_batched(
                || {
                    (
                        ReceiveCipher::new(
                            KEY,
                            SESSION_ID,
                            Direction::InitiatorToResponder,
                            1,
                            LOCAL_RECEIVER_INDEX,
                        ),
                        sealed.clone(),
                    )
                },
                |(mut receiver, mut packets)| {
                    receiver
                        .open_batch_in_place(
                            black_box(&mut packets),
                            black_box(&packet_lengths),
                            black_box(stride),
                        )
                        .expect("batch open benchmark packets")
                },
                BatchSize::PerIteration,
            )
        });

        group.finish();
    }
}

fn bench_epoch_overhead(c: &mut Criterion) {
    use std::time::Instant;

    let plaintext = vec![0x42; 1420];
    let mut group = c.benchmark_group("secure_session_v2_epoch_overhead");
    group.throughput(Throughput::Bytes(plaintext.len() as u64));

    group.bench_function("epoch_seal_1420", |b| {
        let now = Instant::now();
        let send = SendCipher::new(
            KEY,
            SESSION_ID,
            Direction::InitiatorToResponder,
            1,
            LOCAL_RECEIVER_INDEX,
        );
        let receive = ReceiveCipher::new(
            KEY,
            SESSION_ID,
            Direction::ResponderToInitiator,
            1,
            LOCAL_RECEIVER_INDEX,
        );
        let mut session = EpochSession::new(send, receive, now).expect("create epoch fixture");
        let mut packet = vec![0; DATA_HEADER_LEN + plaintext.len() + AEAD_TAG_LEN];
        b.iter(|| {
            session
                .seal(black_box(&plaintext), black_box(&mut packet), now)
                .expect("seal epoch benchmark packet")
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_packet_paths,
    bench_batch_paths,
    bench_epoch_overhead
);
criterion_main!(benches);

use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use fuckipv6_secure_session::{
    configure_crypto_workers, crypto_batch_metrics, Direction, EpochSession, ReceiveCipher,
    SendCipher, SessionError, AEAD_TAG_LEN, DATA_HEADER_LEN, SESSION_ID_LEN,
};
use std::sync::Once;

const KEY: [u8; 32] = [0x5a; 32];
const SESSION_ID: [u8; SESSION_ID_LEN] = [0x33; SESSION_ID_LEN];
const LOCAL_RECEIVER_INDEX: u32 = 0x1122_3344;
const BENCH_WORKER_THREADS: usize = 4;
const BATCH_PACKETS: usize = 64;
const BATCH_PAYLOAD_LENS: [usize; 3] = [64, 1280, 1420];
static CONFIGURE_BENCH_WORKERS: Once = Once::new();

fn configure_bench_workers() {
    CONFIGURE_BENCH_WORKERS.call_once(|| {
        configure_crypto_workers(BENCH_WORKER_THREADS)
            .expect("configure the benchmark crypto worker pool exactly once");
    });
}

fn verify_worker_batch_path() {
    // Criterion executes every benchmark group in one process. Configure the
    // process-wide pool once, then prove that every measured payload size
    // really took the worker path for both seal and open instead of silently
    // falling back to serial execution.
    configure_bench_workers();

    for payload_len in BATCH_PAYLOAD_LENS {
        let plaintext = vec![0x42; payload_len];
        let plaintexts = vec![plaintext.as_slice(); BATCH_PACKETS];
        let stride = DATA_HEADER_LEN + payload_len + AEAD_TAG_LEN;
        let mut packets = vec![0u8; stride * BATCH_PACKETS];
        let mut sender = SendCipher::new(
            KEY,
            SESSION_ID,
            Direction::InitiatorToResponder,
            1,
            LOCAL_RECEIVER_INDEX,
        );

        let before_seal = crypto_batch_metrics();
        sender
            .seal_batch(&plaintexts, &mut packets, stride)
            .expect("initialize benchmark worker pool");
        let after_seal = crypto_batch_metrics();
        assert_eq!(
            after_seal.worker_threads, BENCH_WORKER_THREADS,
            "benchmark worker count must match the explicit configuration"
        );
        assert_eq!(after_seal.failed_worker_initializations, 0);
        assert!(
            after_seal.parallel_jobs > before_seal.parallel_jobs,
            "worker seal path was not selected for {payload_len}B x {BATCH_PACKETS}"
        );

        let packet_lengths = vec![stride; BATCH_PACKETS];
        let mut receiver = ReceiveCipher::new(
            KEY,
            SESSION_ID,
            Direction::InitiatorToResponder,
            1,
            LOCAL_RECEIVER_INDEX,
        );
        let before_open = crypto_batch_metrics();
        receiver
            .open_batch_in_place(&mut packets, &packet_lengths, stride)
            .expect("initialize benchmark worker open path");
        let after_open = crypto_batch_metrics();
        assert!(
            after_open.parallel_jobs > before_open.parallel_jobs,
            "worker open path was not selected for {payload_len}B x {BATCH_PACKETS}"
        );
    }

    eprintln!(
        "PERF-5 batch benchmark: worker_threads={BENCH_WORKER_THREADS}, packets_per_batch={BATCH_PACKETS}, payloads={BATCH_PAYLOAD_LENS:?}, throughput=plaintext_bytes_per_batch"
    );
}

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
    verify_worker_batch_path();

    for payload_len in BATCH_PAYLOAD_LENS {
        let plaintext = vec![0x42; payload_len];
        let plaintexts = vec![plaintext.as_slice(); BATCH_PACKETS];
        let stride = DATA_HEADER_LEN + payload_len + AEAD_TAG_LEN;
        let batch_bytes = stride * BATCH_PACKETS;
        let mut group = c.benchmark_group(format!(
            "secure_session_v2_batch_{payload_len}_w{BENCH_WORKER_THREADS}"
        ));
        group.throughput(Throughput::Bytes((payload_len * BATCH_PACKETS) as u64));

        group.bench_function("serial_seal_x64", |b| {
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
                        .expect("serial seal batch benchmark packet");
                }
                black_box(BATCH_PACKETS)
            })
        });

        group.bench_function("worker_seal_x64", |b| {
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
                    .expect("worker seal batch benchmark packets")
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

        group.bench_function("serial_open_x64", |b| {
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
                            .expect("serial open batch benchmark packet")
                            .len();
                    }
                    black_box(plaintext_bytes)
                },
                BatchSize::PerIteration,
            )
        });

        group.bench_function("worker_open_x64", |b| {
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
                        .expect("worker open batch benchmark packets")
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

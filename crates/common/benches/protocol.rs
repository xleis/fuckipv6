use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use fuckipv6_common::{
    codec, decode_relay_message, encode_relay_message, generate_self_signed_quic_certificate,
    public_key_pin_from_certificate, DataPacket, P2pDatagram, RelayMessage, SignalingMessage,
};
use std::net::Ipv6Addr;

fn bench_protocol_serialization(c: &mut Criterion) {
    let src_ip = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0x10, 1);
    let dst_ip = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0x10, 2);
    let payload = vec![0x60; 1280];
    let packet = DataPacket::new(src_ip, dst_ip, payload.clone());
    let datagram = P2pDatagram::Data(packet);
    let relay = SignalingMessage::RelayPacket {
        src_ip: std::net::IpAddr::V6(src_ip),
        dst_ip: std::net::IpAddr::V6(dst_ip),
        payload: payload.clone(),
    };

    let mut group = c.benchmark_group("protocol_serialization");
    group.throughput(Throughput::Bytes(payload.len() as u64));

    group.bench_function("p2p_datagram_serialize", |b| {
        b.iter(|| codec::encode(black_box(&datagram)).expect("serialize p2p datagram"))
    });

    let datagram_bytes = codec::encode(&datagram).expect("serialize p2p datagram");
    group.bench_function("p2p_datagram_deserialize", |b| {
        b.iter(|| {
            codec::decode::<P2pDatagram>(black_box(&datagram_bytes))
                .expect("deserialize p2p datagram")
        })
    });

    group.bench_function("signaling_relay_serialize", |b| {
        b.iter(|| codec::encode(black_box(&relay)).expect("serialize signaling relay"))
    });

    let relay_bytes = codec::encode(&relay).expect("serialize signaling relay");
    group.bench_function("signaling_relay_deserialize", |b| {
        b.iter(|| {
            codec::decode::<SignalingMessage>(black_box(&relay_bytes))
                .expect("deserialize signaling relay")
        })
    });

    group.finish();
}

fn bench_relay_wire_format(c: &mut Criterion) {
    let src_ip = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0x10, 1);
    let dst_ip = Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0x10, 2);
    let payload = vec![0x42; 1280];
    let packet = RelayMessage::Packet {
        src_ip,
        dst_ip,
        payload,
    };
    let encoded = encode_relay_message(&packet).expect("encode relay packet");

    let mut group = c.benchmark_group("relay_wire_format");
    group.throughput(Throughput::Bytes(encoded.len() as u64));

    group.bench_function("encode_packet", |b| {
        b.iter(|| encode_relay_message(black_box(&packet)).expect("encode relay packet"))
    });

    group.bench_function("decode_packet", |b| {
        b.iter(|| decode_relay_message(black_box(&encoded)).expect("decode relay packet"))
    });

    group.bench_function("encode_decode_packet", |b| {
        b.iter_batched(
            || packet.clone(),
            |message| {
                let encoded = encode_relay_message(black_box(&message)).expect("encode relay");
                decode_relay_message(black_box(&encoded)).expect("decode relay")
            },
            BatchSize::SmallInput,
        )
    });

    group.finish();
}

fn bench_quic_certificate_pin(c: &mut Criterion) {
    let certificate = generate_self_signed_quic_certificate(vec!["fuckipv6.local".to_string()])
        .expect("generate certificate");
    let cert_der = certificate.certificate_der();

    c.bench_function("quic_public_key_pin_from_certificate", |b| {
        b.iter(|| {
            public_key_pin_from_certificate(black_box(&cert_der)).expect("parse public key pin")
        })
    });
}

criterion_group!(
    benches,
    bench_protocol_serialization,
    bench_relay_wire_format,
    bench_quic_certificate_pin
);
criterion_main!(benches);

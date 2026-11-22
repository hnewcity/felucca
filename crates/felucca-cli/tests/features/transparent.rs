use super::*;
use felucca_proto::{StreamRequest, StreamResponse, read_frame, write_frame};

#[tokio::test]
async fn kernel_selected_target_uses_agent_acl_and_half_close_and_cannot_select_virtual_routes() {
    let backend = echo().await;
    for denied in [false, true] {
        let mut config = agent(&format!(
            "[[tunnel]]\nid=1\nmode='transparent'\nport={}",
            free_port().await
        ));
        if denied {
            config.tunnels[0].target_policy.deny = vec!["127.0.0.0/8".into()];
        }
        let services = felucca_service::Services::new(
            &config.tunnels,
            &config.geo,
            Arc::new(
                felucca_transport::network::Network::new(&config.network, config.dial_timeout)
                    .unwrap(),
            ),
        )
        .unwrap();
        let (mut incoming, outgoing) = tokio::io::duplex(64 * 1024);
        let task = tokio::spawn(async move {
            services
                .serve(
                    outgoing,
                    Duration::from_secs(3),
                    &"transparent-test-key".into(),
                )
                .await;
        });
        write_frame(
            &mut incoming,
            &StreamRequest::Routed {
                tunnel_id: 1,
                source: "127.0.0.1".parse().unwrap(),
                target: Some(format!("127.0.0.1:{backend}")),
                datagram: false,
            },
        )
        .await
        .unwrap();
        let response: StreamResponse = read_frame(&mut incoming).await.unwrap();
        if denied {
            assert!(matches!(
                response,
                StreamResponse::Err(felucca_proto::StreamReject::Forbidden)
            ));
        } else {
            assert!(matches!(response, StreamResponse::Ok));
            roundtrip(incoming).await;
        }
        task.await.unwrap();
    }
    let config = agent(&format!(
        "[[tunnel]]\nid=1\nmode='transparent'\nport={}",
        free_port().await
    ));
    let services = felucca_service::Services::new(
        &config.tunnels,
        &config.geo,
        Arc::new(
            felucca_transport::network::Network::new(&config.network, config.dial_timeout).unwrap(),
        ),
    )
    .unwrap();
    let (mut incoming, outgoing) = tokio::io::duplex(4096);
    let task = tokio::spawn(async move {
        services
            .serve(
                outgoing,
                Duration::from_secs(3),
                &"transparent-test-key".into(),
            )
            .await;
    });
    write_frame(
        &mut incoming,
        &StreamRequest::Routed {
            tunnel_id: 1,
            source: "127.0.0.1".parse().unwrap(),
            target: Some("tunnel://other/1".into()),
            datagram: false,
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        read_frame::<_, StreamResponse>(&mut incoming)
            .await
            .unwrap(),
        StreamResponse::Err(felucca_proto::StreamReject::Forbidden)
    ));
    task.await.unwrap();
}

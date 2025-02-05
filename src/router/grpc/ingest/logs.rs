// Copyright 2023 Zinc Labs Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use async_trait::async_trait;
use opentelemetry_proto::tonic::collector::logs::v1::{
    logs_service_client::LogsServiceClient, logs_service_server::LogsService,
    ExportLogsServiceRequest, ExportLogsServiceResponse,
};
use tonic::{
    codec::CompressionEncoding, metadata::MetadataValue, transport::Channel, Request, Response,
    Status,
};
use tracing_opentelemetry::OpenTelemetrySpanExt;

use crate::common::infra::{cluster, config::CONFIG};
use crate::service::search::MetadataMap;

#[derive(Default)]
pub struct LogsServer;

#[async_trait]
impl LogsService for LogsServer {
    async fn export(
        &self,
        request: Request<ExportLogsServiceRequest>,
    ) -> Result<Response<ExportLogsServiceResponse>, Status> {
        let (metadata, extensions, message) = request.into_parts();

        // basic validation
        if !metadata.contains_key(&CONFIG.grpc.org_header_key) {
            return Err(Status::invalid_argument(format!(
                "Please specify organization id with header key '{}' ",
                &CONFIG.grpc.org_header_key
            )));
        }

        // call ingester
        let grpc_addr = super::get_rand_ingester_addr()?;
        let mut request = Request::from_parts(metadata, extensions, message);
        opentelemetry::global::get_text_map_propagator(|propagator| {
            propagator.inject_context(
                &tracing::Span::current().context(),
                &mut MetadataMap(request.metadata_mut()),
            )
        });

        let token: MetadataValue<_> = cluster::get_internal_grpc_token()
            .parse()
            .map_err(|_| Status::internal("invalid token".to_string()))?;
        let channel = Channel::from_shared(grpc_addr.clone())
            .unwrap()
            .connect()
            .await
            .map_err(|err| {
                log::error!(
                    "[ROUTER] grpc->ingest->logs: node: {}, connect err: {:?}",
                    &grpc_addr,
                    err
                );
                Status::internal("connect querier error".to_string())
            })?;
        let client = LogsServiceClient::with_interceptor(channel, move |mut req: Request<()>| {
            req.metadata_mut().insert("authorization", token.clone());
            Ok(req)
        });
        client
            .send_compressed(CompressionEncoding::Gzip)
            .accept_compressed(CompressionEncoding::Gzip)
            .export(request)
            .await
    }
}

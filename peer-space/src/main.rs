mod graphql;

use actix_web::{guard, web, App, HttpResponse, HttpServer};
use async_graphql::http::GraphiQLSource;
use async_graphql_actix_web::GraphQL;

use crate::graphql::generate_schema;

async fn gql_playgound() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(GraphiQLSource::build().endpoint("/").finish())
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    println!("GraphiQL IDE: http://localhost:8000");
    let schema = generate_schema();
    HttpServer::new(move || {
        App::new()
            .service(
                web::resource("/")
                    .guard(guard::Post())
                    .to(GraphQL::new(schema.clone())),
            )
            .service(web::resource("/").guard(guard::Get()).to(gql_playgound))
    })
    .bind("127.0.0.1:8000")?
    .run()
    .await
}

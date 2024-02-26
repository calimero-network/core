use libp2p::identity::Keypair;

fn main() {}

pub struct Post {
    content: String,
}

pub fn start_network(keypair: Option<Keypair>) {
    //call start network endpoint
}

pub fn join_network(app_id: String, keypair: Keypair) {
    // call join network endpoint
}

pub fn login(keypair: Keypair) {
    // generate keypair or use existing one and store it in local storage

    //call fetch challenge endpoint

    //sign challenge

    //call login endpoint
}

pub fn logout() {
    //clean keystore from local storage (if web app)
}

pub fn create_post(post: Post) {
    //call create post endpoint
}

pub fn comment_post(post: Post) {
    //call comment post endpoint
}

pub fn fetch_posts(post: Post) -> Vec<Post> {
    //call fetch posts endpoint
    vec![]
}

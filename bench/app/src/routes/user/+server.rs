// the-benchmarker's `GET /user/:id` (the id back; a handler taking `id`
// serves `/user/[id]`) and `POST /user` (an empty 204).

fn get(id: String) -> Response {
    Response::text(id)
}

fn post() {}

//! Everything a `#[derive(Rest)]` resource does beyond CRUD: filters,
//! sorting, pages, ETags, bulk creates, timestamps, hooks, and a key for
//! deletes (the `CARGO_PKG_NAME` cargo sets when it runs the tests).

#[derive(Rest)]
#[rest(admin = "CARGO_PKG_NAME", table = "tasks")]
struct Task {
    #[validate(len = 1..=40)]
    title: String,
    done: bool,
    points: u32,
    tags: Option<Vec<String>>,
    created_at: String,
    updated_at: u64,
}

fn before_create(task: &mut Task) -> Result {
    if task.title == "forbidden" {
        return invalid("title", "is not allowed");
    }
    task.title = task.title.trim().to_string();
    Ok(())
}

fn before_update(id: u64, task: &Task) -> Result {
    if id == 1 && task.points > 100 {
        return invalid("points", "task 1 is worth 100 at most");
    }
    Ok(())
}

fn after_delete(cx: &mut Cx, task: &Task) {
    cx.set_header("x-deleted", task.title.clone());
}

use crate::notes::{self, Note};

/// What to change; what is left out stays as it is.
#[derive(FromJson)]
struct Changes {
    #[validate(min_len = 1, max_len = 200)]
    title: Option<String>,
    done: Option<bool>,
}

fn get(id: u64) -> Result<Note> {
    notes::find(id).or_404()
}

fn patch(id: u64, body: Changes) -> Result<Note> {
    let note = notes::change(id, body.title, body.done).or_404()?;
    wisp::channel("notes").send(wisp::to_json(&note));
    Ok(note)
}

fn delete(id: u64) -> Result<()> {
    if !notes::remove(id) {
        return error(404, "No such note");
    }
    Ok(())
}

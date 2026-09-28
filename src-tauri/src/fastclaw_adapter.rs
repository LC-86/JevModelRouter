use std::path::Path;
use anyhow::{anyhow, Result};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde_json::{json, Value};

fn open(path: &Path) -> Result<Connection> {
    Ok(Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?)
}
fn scope(db: &Connection) -> Result<&'static str> {
    let cols = db.prepare("PRAGMA table_info(configs)")?.query_map([], |r| r.get::<_,String>(1))?.collect::<Result<Vec<_>,_>>()?;
    if cols.contains(&"scope".into()) {Ok("scope = 'system' AND scope_id = ''")}
    else if cols.contains(&"user_id".into()) && cols.contains(&"agent_id".into()) {Ok("user_id = '' AND agent_id = ''")}
    else {Err(anyhow!("Unsupported FastClaw database schema"))}
}
pub fn connect(path: &Path, port: u16, binding: &str, api: &str) -> Result<()> {
    let mut db=open(path)?;db.busy_timeout(std::time::Duration::from_secs(5))?;
    let filter=scope(&db)?;
    let tx=db.transaction()?;
    // Original rows are captured once in the same transaction as the patch.
    tx.execute_batch("CREATE TABLE IF NOT EXISTS autojev_config_backup (kind TEXT PRIMARY KEY, target_id TEXT NOT NULL, original_data TEXT, original_enabled INTEGER)")?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS autojev_config_owned (kind TEXT PRIMARY KEY, port INTEGER NOT NULL, applied TEXT NOT NULL)")?;
    let model=crate::agent_catalog::wire_id(binding);
    for (kind,name,patch) in [
        ("provider","autojev",json!({"apiBase":format!("http://127.0.0.1:{port}{}",if api=="messages" {""} else {"/v1"}),"apiKey":"autojev-local-fastclaw",
            "apiType":if api=="messages" {"anthropic-messages"} else {"openai-completions"}, "models":[]})),
        ("setting","agents.defaults",json!({"model":format!("autojev/{model}")})),
    ] {
        let existing:Option<(String,String,i64)>=tx.query_row(&format!("SELECT id,data,enabled FROM configs WHERE {filter} AND kind=?1 AND name=?2"),params![kind,name],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        let id=existing.as_ref().map(|r|r.0.clone()).unwrap_or_else(||uuid::Uuid::new_v4().to_string());
        let mut data:Value=existing.as_ref().map(|r|serde_json::from_str(&r.1)).transpose()?.unwrap_or(json!({}));
        let obj=data.as_object_mut().ok_or_else(||anyhow!("Invalid FastClaw configuration"))?;
        for (key,value) in patch.as_object().unwrap() {obj.insert(key.clone(),value.clone());}
        let previous:Option<String>=tx.query_row("SELECT applied FROM autojev_config_owned WHERE kind=?1",[kind],|r|r.get(0)).optional()?;
        if let Some(previous)=previous {
            let old:Option<String>=tx.query_row("SELECT original_data FROM autojev_config_backup WHERE kind=?1",[kind],|r|r.get(0))?;
            let before=old.as_deref().map(serde_json::from_str::<Value>).transpose()?;
            let applied:Value=serde_json::from_str(&previous)?;
            let current=existing.as_ref().map(|r|serde_json::from_str::<Value>(&r.1)).transpose()?;
            let rebased=crate::ownership::revert_model_configuration(before.as_ref(),&applied,current.as_ref(),&[]);
            tx.execute("UPDATE autojev_config_backup SET target_id=?1,original_data=?2,original_enabled=CASE WHEN ?3 IS NULL THEN NULL WHEN ?3 != 1 THEN ?3 ELSE original_enabled END WHERE kind=?4",params![id,rebased.map(|v|v.to_string()),existing.as_ref().map(|r|r.2),kind])?;
        }
        tx.execute("INSERT OR IGNORE INTO autojev_config_backup(kind,target_id,original_data,original_enabled) VALUES(?1,?2,?3,?4)",
            params![kind,id,existing.as_ref().map(|r|&r.1),existing.as_ref().map(|r|r.2)])?;
        tx.execute("INSERT OR REPLACE INTO autojev_config_owned(kind,port,applied) VALUES(?1,?2,?3)",params![kind,port,data.to_string()])?;
        if existing.is_some() {
            tx.execute("UPDATE configs SET data=?1,enabled=1,updated_at=CURRENT_TIMESTAMP WHERE id=?2",params![data.to_string(),id])?;
        } else {
            let columns=if filter.starts_with("scope ") {"scope,scope_id"} else {"user_id,agent_id"};
            let values=if filter.starts_with("scope ") {"'system',''"} else {"'',''"};
            tx.execute(&format!("INSERT INTO configs(id,kind,{columns},name,enabled,data) VALUES(?1,?2,{values},?3,1,?4)"),params![id,kind,name,data.to_string()])?;
        }
    }
    tx.commit()?;Ok(())
}
pub fn restore(path: &Path) -> Result<()> {
    let mut db=open(path)?;let tx=db.transaction()?;
    let saved=tx.prepare("SELECT target_id,original_data,original_enabled,kind FROM autojev_config_backup")?
        .query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<i64>>(2)?,r.get::<_,String>(3)?)))?.collect::<Result<Vec<_>,_>>()?;
    if saved.is_empty(){return Err(anyhow!("No AutoJev backup exists"));}
    for (id,mut data,enabled,kind) in saved {
        let mut applied:Option<String>=tx.query_row("SELECT o.applied FROM autojev_config_owned o JOIN autojev_config_backup b ON o.kind=b.kind WHERE b.target_id=?1",[&id],|r|r.get(0)).optional().unwrap_or(None);
        if applied.is_none() {
            let current:Option<String>=tx.query_row("SELECT data FROM configs WHERE id=?1",[&id],|r|r.get(0)).optional()?;
            if let Some(current)=current {
                let old=data.as_deref().map(serde_json::from_str::<Value>).transpose()?.unwrap_or(json!({}));
                let mut before:Value=serde_json::from_str(&current)?;
                let keys=if kind=="provider"{vec!["apiBase","apiKey","apiType","models"]}else{vec!["model"]};
                let object=before.as_object_mut().ok_or_else(||anyhow!("Invalid FastClaw configuration"))?;
                for key in keys {if let Some(value)=old.get(key){object.insert(key.into(),value.clone());}else{object.remove(key);}}
                data=if object.is_empty(){None}else{Some(before.to_string())};applied=Some(current);
            }
        }
        if let Some(applied)=applied {
            let current:Option<(String,i64)>=tx.query_row("SELECT data,enabled FROM configs WHERE id=?1",[&id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            if let Some((current,current_enabled))=current {
                let before=data.as_deref().map(serde_json::from_str::<Value>).transpose()?;
                let a:Value=serde_json::from_str(&applied)?;let c:Value=serde_json::from_str(&current)?;
                if let Some(restored)=crate::ownership::revert_model_configuration(before.as_ref(),&a,Some(&c),&[]) {
                    if restored.get("apiBase").and_then(Value::as_str).and_then(|s|s.parse::<reqwest::Url>().ok()).is_some_and(|u|u.host_str()==Some("127.0.0.1") && u.port().is_some() && u.port()==a.get("apiBase").and_then(Value::as_str).and_then(|s|s.parse::<reqwest::Url>().ok()).and_then(|v|v.port())) {return Err(anyhow!("FastClaw gateway endpoint could not be restored"));}
                    tx.execute("UPDATE configs SET data=?1,enabled=?2 WHERE id=?3",params![restored.to_string(),if current_enabled==1{enabled.unwrap_or(1)}else{current_enabled},id])?;
                }else{tx.execute("DELETE FROM configs WHERE id=?1",[&id])?;}
            }
            continue;
        }
        if let Some(data)=data {tx.execute("UPDATE configs SET data=?1,enabled=?2 WHERE id=?3",params![data,enabled,id])?;}
        else {tx.execute("DELETE FROM configs WHERE id=?1",[id])?;}
    }
    let _=tx.execute("DELETE FROM autojev_config_owned",[]);
    tx.execute("DELETE FROM autojev_config_backup",[])?;tx.commit()?;Ok(())
}
pub fn binding(path:&Path)->Option<String> {
    let db=Connection::open_with_flags(path,OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    let filter=scope(&db).ok()?;
    let data:String=db.query_row(&format!("SELECT data FROM configs WHERE {filter} AND kind='setting' AND name='agents.defaults' AND enabled=1"),[],|r|r.get(0)).ok()?;
    let provider:String=db.query_row(&format!("SELECT data FROM configs WHERE {filter} AND kind='provider' AND name='autojev' AND enabled=1"),[],|r|r.get(0)).ok()?;
    if !serde_json::from_str::<Value>(&provider).ok()?.get("apiBase")?.as_str()?.starts_with("http://127.0.0.1:"){return None;}
    serde_json::from_str::<Value>(&data).ok()?.get("model")?.as_str()?.strip_prefix("autojev/").map(str::to_owned)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn scopes_and_restore() {
        for modern in [false,true] {
            let dir=tempfile::tempdir().unwrap();let path=dir.path().join("fastclaw.db");let db=Connection::open(&path).unwrap();
            let columns=if modern {"user_id TEXT DEFAULT '',agent_id TEXT DEFAULT ''"} else {"scope TEXT DEFAULT 'system',scope_id TEXT DEFAULT ''"};
            db.execute_batch(&format!("CREATE TABLE configs(id TEXT PRIMARY KEY,kind TEXT,{columns},name TEXT,enabled INTEGER,data TEXT,updated_at TEXT)")).unwrap();
            db.execute("INSERT INTO configs(id,kind,name,enabled,data) VALUES('old','setting','agents.defaults',1,?1)",[r#"{"model":"original/model","maxTokens":4096}"#]).unwrap();
            connect(&path,9526,"model/test","chat_completions").unwrap();
            let provider: String = db.query_row("SELECT data FROM configs WHERE kind='provider' AND name='autojev'", [], |r| r.get(0)).unwrap();
            assert_eq!(serde_json::from_str::<Value>(&provider).unwrap()["apiKey"], "autojev-local-fastclaw");
            assert_eq!(binding(&path).as_deref(),Some("model/test"));
            // Changes to unrelated rows after connection must survive restore.
            db.execute("INSERT INTO configs(id,kind,name,enabled,data) VALUES('unrelated','provider','other',1,'{}')",[]).unwrap();
            db.execute("UPDATE configs SET data=?1 WHERE id='old'",[r#"{"model":"autojev/provider/other","maxTokens":8192,"userAdded":true}"#]).unwrap();
            connect(&path,9526,"new","messages").unwrap();
            restore(&path).unwrap();
            let data:String=db.query_row("SELECT data FROM configs WHERE id='old'",[],|r|r.get(0)).unwrap();
            let restored:Value=serde_json::from_str(&data).unwrap();
            assert_eq!(restored["model"],"original/model");assert_eq!(restored["maxTokens"],8192);assert_eq!(restored["userAdded"],true);
            assert_eq!(db.query_row("SELECT count(*) FROM configs WHERE id='unrelated'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
            assert!(binding(&path).is_none());
        }
    }
}

pub fn uses_port(path:&Path,port:u16)->bool {
    let Ok(db)=open(path) else{return false;};let Ok(filter)=scope(&db) else{return false;};
    let data:Option<String>=db.query_row(&format!("SELECT data FROM configs WHERE {filter} AND kind='provider' AND name='autojev'"),[],|r|r.get(0)).ok();
    data.and_then(|s|serde_json::from_str::<Value>(&s).ok()).and_then(|v|v.get("apiBase").and_then(Value::as_str).map(str::to_owned)).and_then(|s|s.parse::<reqwest::Url>().ok()).is_some_and(|u|u.host_str()==Some("127.0.0.1")&&u.port()==Some(port))
}

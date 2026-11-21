//! 대시보드 사용자와 비밀번호 해시(argon2id).

use sqlx::PgPool;

/// 새 사용자. `password_hash`는 [`hash_password`] 결과(PHC 문자열)다.
#[derive(Debug, Clone)]
pub struct NewUser {
    pub email: String,
    pub password_hash: String,
    pub name: String,
    pub is_super_admin: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: i64,
    pub email: String,
    pub name: String,
    pub is_super_admin: bool,
}

pub fn hash_password(_password: &str) -> anyhow::Result<String> {
    todo!("WP3")
}

pub async fn create(_pool: &PgPool, _u: &NewUser) -> anyhow::Result<User> {
    todo!("WP3")
}

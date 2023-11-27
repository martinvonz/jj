use jj_lib::backend::MillisSinceEpoch;
use jj_lib::backend::Signature;
use jj_lib::backend::Timestamp;
use jj_lib::repo::Repo;
use jj_lib::settings::UserSettings;
use jj_lib::signing::SigStatus;
use jj_lib::signing::SignBehavior;
use jj_lib::signing::Signer;
use jj_lib::signing::Verification;
use jj_lib::test_signing_backend::TestSigningBackend;
use test_case::test_case;
use testutils::create_random_commit;
use testutils::write_random_commit;
use testutils::TestRepoBackend;
use testutils::TestWorkspace;

fn user_settings(sign_all: bool) -> UserSettings {
    let config = testutils::base_config()
        .add_source(config::File::from_str(
            &format!(
                r#"
                signing.key = "impeccable"
                signing.sign-all = {sign_all}
                "#
            ),
            config::FileFormat::Toml,
        ))
        .build()
        .unwrap();
    UserSettings::from_config(config)
}

fn someone_else() -> Signature {
    Signature {
        name: "Someone Else".to_string(),
        email: "someone-else@example.com".to_string(),
        timestamp: Timestamp {
            timestamp: MillisSinceEpoch(0),
            tz_offset: 0,
        },
    }
}

fn good_verification() -> Option<Verification> {
    Some(Verification::new(
        SigStatus::Good,
        Some("impeccable".to_owned()),
        None,
        Some("test".into()),
    ))
}

#[test_case(TestRepoBackend::Local ; "local backend")]
#[test_case(TestRepoBackend::Git ; "git backend")]
fn manual(backend: TestRepoBackend) {
    let settings = user_settings(true);

    let signer = Signer::new(Some(Box::new(TestSigningBackend)), vec![]);
    let test_workspace = TestWorkspace::init_with_backend_and_signer(&settings, backend, signer);

    let repo = &test_workspace.repo;

    let settings = settings.clone();
    let repo = repo.clone();
    let mut tx = repo.start_transaction(&settings);
    let commit1 = create_random_commit(tx.repo_mut(), &settings)
        .set_sign_behavior(SignBehavior::Own)
        .write()
        .unwrap();
    let commit2 = create_random_commit(tx.repo_mut(), &settings)
        .set_sign_behavior(SignBehavior::Own)
        .set_author(someone_else())
        .write()
        .unwrap();
    tx.commit("test").unwrap();

    let commit1 = repo.store().get_commit(commit1.id()).unwrap();
    assert_eq!(commit1.verification().unwrap(), good_verification());

    let commit2 = repo.store().get_commit(commit2.id()).unwrap();
    assert_eq!(commit2.verification().unwrap(), None);
}

#[test_case(TestRepoBackend::Git ; "git backend")]
fn keep_on_rewrite(backend: TestRepoBackend) {
    let settings = user_settings(true);

    let signer = Signer::new(Some(Box::new(TestSigningBackend)), vec![]);
    let test_workspace = TestWorkspace::init_with_backend_and_signer(&settings, backend, signer);

    let repo = &test_workspace.repo;

    let settings = settings.clone();
    let repo = repo.clone();
    let mut tx = repo.start_transaction(&settings);
    let commit = create_random_commit(tx.repo_mut(), &settings)
        .set_sign_behavior(SignBehavior::Own)
        .write()
        .unwrap();
    tx.commit("test").unwrap();

    let mut tx = repo.start_transaction(&settings);
    let mut_repo = tx.repo_mut();
    let rewritten = mut_repo.rewrite_commit(&settings, &commit).write().unwrap();

    let commit = repo.store().get_commit(rewritten.id()).unwrap();
    assert_eq!(commit.verification().unwrap(), good_verification());
}

#[test_case(TestRepoBackend::Git ; "git backend")]
fn manual_drop_on_rewrite(backend: TestRepoBackend) {
    let settings = user_settings(true);

    let signer = Signer::new(Some(Box::new(TestSigningBackend)), vec![]);
    let test_workspace = TestWorkspace::init_with_backend_and_signer(&settings, backend, signer);

    let repo = &test_workspace.repo;

    let settings = settings.clone();
    let repo = repo.clone();
    let mut tx = repo.start_transaction(&settings);
    let commit = create_random_commit(tx.repo_mut(), &settings)
        .set_sign_behavior(SignBehavior::Own)
        .write()
        .unwrap();
    tx.commit("test").unwrap();

    let mut tx = repo.start_transaction(&settings);
    let mut_repo = tx.repo_mut();
    let rewritten = mut_repo
        .rewrite_commit(&settings, &commit)
        .set_sign_behavior(SignBehavior::Drop)
        .write()
        .unwrap();

    let commit = repo.store().get_commit(rewritten.id()).unwrap();
    assert_eq!(commit.verification().unwrap(), None);
}

#[test_case(TestRepoBackend::Git ; "git backend")]
fn forced(backend: TestRepoBackend) {
    let settings = user_settings(true);

    let signer = Signer::new(Some(Box::new(TestSigningBackend)), vec![]);
    let test_workspace = TestWorkspace::init_with_backend_and_signer(&settings, backend, signer);

    let repo = &test_workspace.repo;

    let settings = settings.clone();
    let repo = repo.clone();
    let mut tx = repo.start_transaction(&settings);
    let commit = create_random_commit(tx.repo_mut(), &settings)
        .set_sign_behavior(SignBehavior::Force)
        .set_author(someone_else())
        .write()
        .unwrap();
    tx.commit("test").unwrap();

    let commit = repo.store().get_commit(commit.id()).unwrap();
    assert_eq!(commit.verification().unwrap(), good_verification());
}

#[test_case(TestRepoBackend::Git ; "git backend")]
fn configured(backend: TestRepoBackend) {
    let settings = user_settings(true);

    let signer = Signer::new(Some(Box::new(TestSigningBackend)), vec![]);
    let test_workspace = TestWorkspace::init_with_backend_and_signer(&settings, backend, signer);

    let repo = &test_workspace.repo;

    let settings = settings.clone();
    let repo = repo.clone();
    let mut tx = repo.start_transaction(&settings);
    let commit = write_random_commit(tx.repo_mut(), &settings);
    tx.commit("test").unwrap();

    let commit = repo.store().get_commit(commit.id()).unwrap();
    assert_eq!(commit.verification().unwrap(), good_verification());
}

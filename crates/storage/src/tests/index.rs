use super::*;
use crate::store::MainStorage;

mod index__public_methods {
    use super::*;

    #[test]
    fn add_child_to() {
        let root_id = Id::random();
        let root_hash = [1_u8; 32];

        assert!(<Index<MainStorage>>::add_root(ChildInfo::new(
            root_id,
            root_hash,
            Metadata::default()
        ),)
        .is_ok());

        let root_index = <Index<MainStorage>>::get_index(root_id).unwrap().unwrap();
        assert_eq!(root_index.id, root_id);
        assert_eq!(root_index.own_hash, root_hash);
        assert!(root_index.parent_id.is_none());
        assert!(root_index.children.is_empty());

        let collection_name = "Books";
        let child_id = Id::random();
        let child_own_hash = [2_u8; 32];
        let child_full_hash: [u8; 32] =
            hex::decode("75877bb41d393b5fb8455ce60ecd8dda001d06316496b14dfa7f895656eeca4a")
                .unwrap()
                .try_into()
                .unwrap();

        assert!(<Index<MainStorage>>::add_child_to(
            root_id,
            collection_name,
            ChildInfo::new(child_id, child_own_hash, Metadata::default()),
        )
        .is_ok());

        let updated_root_index = <Index<MainStorage>>::get_index(root_id).unwrap().unwrap();
        assert_eq!(updated_root_index.id, root_id);
        assert_eq!(updated_root_index.own_hash, root_hash);
        assert!(updated_root_index.parent_id.is_none());
        assert_eq!(updated_root_index.children.len(), 1);
        assert_eq!(
            updated_root_index.children[collection_name][0],
            ChildInfo::new(child_id, child_full_hash, Metadata::default())
        );

        let child_index = <Index<MainStorage>>::get_index(child_id).unwrap().unwrap();
        assert_eq!(child_index.id, child_id);
        assert_eq!(child_index.own_hash, child_own_hash);
        assert_eq!(child_index.parent_id, Some(root_id));
        assert!(child_index.children.is_empty());
    }

    #[test]
    fn add_root() {
        let root_id = Id::random();
        let root_hash = [1_u8; 32];

        assert!(<Index<MainStorage>>::add_root(ChildInfo::new(
            root_id,
            root_hash,
            Metadata::default()
        ),)
        .is_ok());

        let root_index = <Index<MainStorage>>::get_index(root_id).unwrap().unwrap();
        assert_eq!(root_index.id, root_id);
        assert_eq!(root_index.own_hash, root_hash);
        assert!(root_index.parent_id.is_none());
        assert!(root_index.children.is_empty());
    }

    #[test]
    fn get_ancestors_of() {
        let root_id = Id::random();
        let root_hash = [1_u8; 32];
        let child_collection_name = "Books";
        let grandchild_collection_name = "Pages";
        let greatgrandchild_collection_name = "Paragraphs";

        assert!(<Index<MainStorage>>::add_root(ChildInfo::new(
            root_id,
            root_hash,
            Metadata::default()
        ),)
        .is_ok());

        let child_id = Id::random();
        let child_hash = [2_u8; 32];
        let child_info = ChildInfo::new(child_id, child_hash, Metadata::default());
        assert!(
            <Index<MainStorage>>::add_child_to(root_id, child_collection_name, child_info).is_ok()
        );

        let grandchild_id = Id::random();
        let grandchild_hash = [3_u8; 32];
        let grandchild_info = ChildInfo::new(grandchild_id, grandchild_hash, Metadata::default());
        assert!(<Index<MainStorage>>::add_child_to(
            child_id,
            grandchild_collection_name,
            grandchild_info,
        )
        .is_ok());

        let greatgrandchild_id = Id::random();
        let greatgrandchild_hash = [4_u8; 32];
        let greatgrandchild_info = ChildInfo::new(
            greatgrandchild_id,
            greatgrandchild_hash,
            Metadata::default(),
        );
        assert!(<Index<MainStorage>>::add_child_to(
            grandchild_id,
            greatgrandchild_collection_name,
            greatgrandchild_info,
        )
        .is_ok());

        let ancestors = <Index<MainStorage>>::get_ancestors_of(greatgrandchild_id).unwrap();
        assert_eq!(ancestors.len(), 3);
        assert_eq!(
            ancestors[0],
            ChildInfo::new(
                grandchild_id,
                <Index<MainStorage>>::get_hashes_for(grandchild_id)
                    .unwrap()
                    .unwrap()
                    .0,
                Metadata::default()
            )
        );
        assert_eq!(
            ancestors[1],
            ChildInfo::new(
                child_id,
                <Index<MainStorage>>::get_hashes_for(child_id)
                    .unwrap()
                    .unwrap()
                    .0,
                Metadata::default()
            )
        );
        assert_eq!(
            ancestors[2],
            ChildInfo::new(
                root_id,
                <Index<MainStorage>>::get_hashes_for(root_id)
                    .unwrap()
                    .unwrap()
                    .0,
                Metadata::default()
            )
        );
    }

    #[test]
    fn get_children_of__single_collection() {
        let root_id = Id::random();
        let root_hash = [1_u8; 32];

        assert!(<Index<MainStorage>>::add_root(ChildInfo::new(
            root_id,
            root_hash,
            Metadata::default()
        ),)
        .is_ok());

        let collection_name = "Books";
        let child1_id = Id::from([2; 32]);
        let child1_own_hash = [2_u8; 32];
        let child1_full_hash: [u8; 32] =
            hex::decode("75877bb41d393b5fb8455ce60ecd8dda001d06316496b14dfa7f895656eeca4a")
                .unwrap()
                .try_into()
                .unwrap();

        let child2_id = Id::from([3; 32]);
        let child2_own_hash = [3_u8; 32];
        let child2_full_hash: [u8; 32] =
            hex::decode("648aa5c579fb30f38af744d97d6ec840c7a91277a499a0d780f3e7314eca090b")
                .unwrap()
                .try_into()
                .unwrap();

        assert!(<Index<MainStorage>>::add_child_to(
            root_id,
            collection_name,
            ChildInfo::new(child1_id, child1_own_hash, Metadata::default()),
        )
        .is_ok());
        assert!(<Index<MainStorage>>::add_child_to(
            root_id,
            collection_name,
            ChildInfo::new(child2_id, child2_own_hash, Metadata::default()),
        )
        .is_ok());

        let children = <Index<MainStorage>>::get_children_of(root_id, collection_name).unwrap();
        assert_eq!(children.len(), 2);
        assert_eq!(
            children[0],
            ChildInfo::new(child1_id, child1_full_hash, Metadata::default())
        );
        assert_eq!(
            children[1],
            ChildInfo::new(child2_id, child2_full_hash, Metadata::default())
        );
    }

    #[test]
    fn get_children_of__two_collections() {
        let root_id = Id::from([1; 32]);
        let root_hash = [1_u8; 32];

        assert!(<Index<MainStorage>>::add_root(ChildInfo::new(
            root_id,
            root_hash,
            Metadata::default()
        ),)
        .is_ok());

        let collection1_name = "Pages";
        let child1_id = Id::from([2; 32]);
        let child1_own_hash = [2_u8; 32];
        let child1_full_hash: [u8; 32] =
            hex::decode("75877bb41d393b5fb8455ce60ecd8dda001d06316496b14dfa7f895656eeca4a")
                .unwrap()
                .try_into()
                .unwrap();
        let child2_id = Id::from([3; 32]);
        let child2_own_hash = [3_u8; 32];
        let child2_full_hash: [u8; 32] =
            hex::decode("648aa5c579fb30f38af744d97d6ec840c7a91277a499a0d780f3e7314eca090b")
                .unwrap()
                .try_into()
                .unwrap();

        let collection2_name = "Reviews";
        let child3_id = Id::from([4; 32]);
        let child3_own_hash = [4_u8; 32];
        let child3_full_hash: [u8; 32] =
            hex::decode("9f4fb68f3e1dac82202f9aa581ce0bbf1f765df0e9ac3c8c57e20f685abab8ed")
                .unwrap()
                .try_into()
                .unwrap();

        assert!(<Index<MainStorage>>::add_child_to(
            root_id,
            collection1_name,
            ChildInfo::new(child1_id, child1_own_hash, Metadata::default()),
        )
        .is_ok());
        assert!(<Index<MainStorage>>::add_child_to(
            root_id,
            collection1_name,
            ChildInfo::new(child2_id, child2_own_hash, Metadata::default()),
        )
        .is_ok());
        assert!(<Index<MainStorage>>::add_child_to(
            root_id,
            collection2_name,
            ChildInfo::new(child3_id, child3_own_hash, Metadata::default()),
        )
        .is_ok());

        let children1 = <Index<MainStorage>>::get_children_of(root_id, collection1_name).unwrap();
        assert_eq!(children1.len(), 2);
        assert_eq!(
            children1[0],
            ChildInfo::new(child1_id, child1_full_hash, Metadata::default())
        );
        assert_eq!(
            children1[1],
            ChildInfo::new(child2_id, child2_full_hash, Metadata::default())
        );
        let children2 = <Index<MainStorage>>::get_children_of(root_id, collection2_name).unwrap();
        assert_eq!(children2.len(), 1);
        assert_eq!(
            children2[0],
            ChildInfo::new(child3_id, child3_full_hash, Metadata::default())
        );
    }

    #[test]
    fn get_collection_names_for() {
        let root_id = Id::random();
        let root_hash = [1_u8; 32];

        assert!(<Index<MainStorage>>::add_root(ChildInfo::new(
            root_id,
            root_hash,
            Metadata::default()
        ),)
        .is_ok());

        let collection1_name = "Pages";
        let collection2_name = "Chapters";
        let mut collection_names = vec![collection1_name.to_owned(), collection2_name.to_owned()];
        collection_names.sort();
        let child1_id = Id::random();
        let child1_own_hash = [2_u8; 32];
        let child2_id = Id::random();
        let child2_own_hash = [3_u8; 32];

        assert!(<Index<MainStorage>>::add_child_to(
            root_id,
            collection1_name,
            ChildInfo::new(child1_id, child1_own_hash, Metadata::default()),
        )
        .is_ok());
        assert!(<Index<MainStorage>>::add_child_to(
            root_id,
            collection2_name,
            ChildInfo::new(child2_id, child2_own_hash, Metadata::default()),
        )
        .is_ok());

        assert_eq!(
            <Index<MainStorage>>::get_collection_names_for(root_id).unwrap(),
            collection_names
        );
    }

    #[test]
    fn get_hashes_for() {
        let root_id = Id::new([0_u8; 32]);
        let root_own_hash = [1_u8; 32];
        let root_full_hash = [0_u8; 32];

        assert!(<Index<MainStorage>>::add_root(ChildInfo::new(
            root_id,
            root_own_hash,
            Metadata::default()
        ),)
        .is_ok());

        assert_eq!(
            <Index<MainStorage>>::get_hashes_for(root_id)
                .unwrap()
                .unwrap(),
            (root_full_hash, root_own_hash)
        );
    }

    #[test]
    fn get_parent_id() {
        let root_id = Id::random();
        let root_hash = [1_u8; 32];

        assert!(<Index<MainStorage>>::add_root(ChildInfo::new(
            root_id,
            root_hash,
            Metadata::default()
        ),)
        .is_ok());

        let root_index = <Index<MainStorage>>::get_index(root_id).unwrap().unwrap();
        assert_eq!(root_index.id, root_id);
        assert_eq!(root_index.own_hash, root_hash);
        assert!(root_index.parent_id.is_none());
        assert!(root_index.children.is_empty());

        let collection_name = "Books";
        let child_id = Id::random();
        let child_own_hash = [2_u8; 32];

        assert!(<Index<MainStorage>>::add_child_to(
            root_id,
            collection_name,
            ChildInfo::new(child_id, child_own_hash, Metadata::default()),
        )
        .is_ok());

        assert_eq!(
            <Index<MainStorage>>::get_parent_id(child_id).unwrap(),
            Some(root_id)
        );
        assert_eq!(<Index<MainStorage>>::get_parent_id(root_id).unwrap(), None);
    }

    #[test]
    fn has_children() {
        let root_id = Id::random();
        let root_hash = [1_u8; 32];
        let collection_name = "Books";

        assert!(<Index<MainStorage>>::add_root(ChildInfo::new(
            root_id,
            root_hash,
            Metadata::default()
        ),)
        .is_ok());
        assert!(!<Index<MainStorage>>::has_children(root_id, collection_name).unwrap());

        let child_id = Id::random();
        let child_own_hash = [2_u8; 32];

        assert!(<Index<MainStorage>>::add_child_to(
            root_id,
            collection_name,
            ChildInfo::new(child_id, child_own_hash, Metadata::default()),
        )
        .is_ok());
        assert!(<Index<MainStorage>>::has_children(root_id, collection_name).unwrap());
    }

    #[test]
    fn remove_child_from() {
        let root_id = Id::random();
        let root_hash = [1_u8; 32];

        assert!(<Index<MainStorage>>::add_root(ChildInfo::new(
            root_id,
            root_hash,
            Metadata::default()
        ),)
        .is_ok());

        let root_index = <Index<MainStorage>>::get_index(root_id).unwrap().unwrap();
        assert_eq!(root_index.id, root_id);
        assert_eq!(root_index.own_hash, root_hash);
        assert!(root_index.parent_id.is_none());
        assert!(root_index.children.is_empty());

        let collection_name = "Books";
        let child_id = Id::random();
        let child_own_hash = [2_u8; 32];

        assert!(<Index<MainStorage>>::add_child_to(
            root_id,
            collection_name,
            ChildInfo::new(child_id, child_own_hash, Metadata::default()),
        )
        .is_ok());
        assert!(
            <Index<MainStorage>>::remove_child_from(root_id, collection_name, child_id).is_ok()
        );

        let root_index = <Index<MainStorage>>::get_index(root_id).unwrap().unwrap();
        assert!(root_index.children[collection_name].is_empty());
        assert!(<Index<MainStorage>>::get_index(child_id).unwrap().is_none());
    }
}

mod index__private_methods {
    use super::*;

    #[test]
    fn get_and_save_index() {
        let id = Id::random();
        let hash1 = [1_u8; 32];
        let hash2 = [2_u8; 32];
        assert!(<Index<MainStorage>>::get_index(id).unwrap().is_none());

        let index = EntityIndex {
            id,
            parent_id: None,
            children: BTreeMap::new(),
            full_hash: hash1,
            own_hash: hash2,
            metadata: Metadata::default(),
        };
        <Index<MainStorage>>::save_index(&index).unwrap();

        assert_eq!(<Index<MainStorage>>::get_index(id).unwrap().unwrap(), index);
    }

    #[test]
    fn save_and_remove_index() {
        let id = Id::random();
        let hash1 = [1_u8; 32];
        let hash2 = [2_u8; 32];
        assert!(<Index<MainStorage>>::get_index(id).unwrap().is_none());

        let index = EntityIndex {
            id,
            parent_id: None,
            children: BTreeMap::new(),
            full_hash: hash1,
            own_hash: hash2,
            metadata: Metadata::default(),
        };
        <Index<MainStorage>>::save_index(&index).unwrap();
        assert_eq!(<Index<MainStorage>>::get_index(id).unwrap().unwrap(), index);

        <Index<MainStorage>>::remove_index(id);
        assert!(<Index<MainStorage>>::get_index(id).unwrap().is_none());
    }
}

#[cfg(test)]
mod hashing {
    use super::*;

    #[test]
    fn calculate_full_merkle_hash_for__with_children() {
        let root_id = Id::from([0; 32]);
        assert!(<Index<MainStorage>>::add_root(ChildInfo::new(
            root_id,
            [0_u8; 32],
            Metadata::default()
        ),)
        .is_ok());

        let collection_name = "Children";
        let child1_id = Id::from([1; 32]);
        let child1_hash = [1_u8; 32];
        let child1_info = ChildInfo::new(child1_id, child1_hash, Metadata::default());
        assert!(<Index<MainStorage>>::add_child_to(root_id, collection_name, child1_info).is_ok());
        let child2_id = Id::from([2; 32]);
        let child2_hash = [2_u8; 32];
        let child2_info = ChildInfo::new(child2_id, child2_hash, Metadata::default());
        assert!(<Index<MainStorage>>::add_child_to(root_id, collection_name, child2_info).is_ok());
        let child3_id = Id::from([3; 32]);
        let child3_hash = [3_u8; 32];
        let child3_info = ChildInfo::new(child3_id, child3_hash, Metadata::default());
        assert!(<Index<MainStorage>>::add_child_to(root_id, collection_name, child3_info).is_ok());

        assert_eq!(
            hex::encode(
                <Index<MainStorage>>::calculate_full_merkle_hash_for(child1_id, false).unwrap()
            ),
            "72cd6e8422c407fb6d098690f1130b7ded7ec2f7f5e1d30bd9d521f015363793",
        );
        assert_eq!(
            hex::encode(
                <Index<MainStorage>>::calculate_full_merkle_hash_for(child2_id, false).unwrap()
            ),
            "75877bb41d393b5fb8455ce60ecd8dda001d06316496b14dfa7f895656eeca4a",
        );
        assert_eq!(
            hex::encode(
                <Index<MainStorage>>::calculate_full_merkle_hash_for(child3_id, false).unwrap()
            ),
            "648aa5c579fb30f38af744d97d6ec840c7a91277a499a0d780f3e7314eca090b",
        );
        assert_eq!(
            hex::encode(
                <Index<MainStorage>>::calculate_full_merkle_hash_for(root_id, false).unwrap()
            ),
            "866edea6f7ce51612ad0ea3bcde93b2494d77e8c466bc2a69817a6443f2a57f0",
        );
    }

    #[test]
    fn recalculate_ancestor_hashes_for() {
        let root_id = Id::random();
        let root_hash = [1_u8; 32];
        let child_collection_name = "Books";
        let grandchild_collection_name = "Pages";
        let greatgrandchild_collection_name = "Paragraphs";

        assert!(<Index<MainStorage>>::add_root(ChildInfo::new(
            root_id,
            root_hash,
            Metadata::default()
        ),)
        .is_ok());

        let root_index = <Index<MainStorage>>::get_index(root_id).unwrap().unwrap();
        assert_eq!(root_index.full_hash, [0_u8; 32]);

        let child_id = Id::random();
        let child_hash = [2_u8; 32];
        let child_info = ChildInfo::new(child_id, child_hash, Metadata::default());
        assert!(
            <Index<MainStorage>>::add_child_to(root_id, child_collection_name, child_info).is_ok()
        );

        let root_index_with_child = <Index<MainStorage>>::get_index(root_id).unwrap().unwrap();
        let child_index = <Index<MainStorage>>::get_index(child_id).unwrap().unwrap();
        assert_eq!(
            hex::encode(root_index_with_child.full_hash),
            "3f18867aec61c1c3cd3ca1b8a0ff42612a8dd0ad83f3e59055e3b9ba737e31d9"
        );
        assert_eq!(
            hex::encode(child_index.full_hash),
            "75877bb41d393b5fb8455ce60ecd8dda001d06316496b14dfa7f895656eeca4a"
        );

        let grandchild_id = Id::random();
        let grandchild_hash = [3_u8; 32];
        let grandchild_info = ChildInfo::new(grandchild_id, grandchild_hash, Metadata::default());
        assert!(<Index<MainStorage>>::add_child_to(
            child_id,
            grandchild_collection_name,
            grandchild_info,
        )
        .is_ok());

        let root_index_with_grandchild = <Index<MainStorage>>::get_index(root_id).unwrap().unwrap();
        let child_index_with_grandchild =
            <Index<MainStorage>>::get_index(child_id).unwrap().unwrap();
        let grandchild_index = <Index<MainStorage>>::get_index(grandchild_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            hex::encode(root_index_with_grandchild.full_hash),
            "2504baa308dcb51f7046815258e36cd4a83d34c6b1d5f1cc1b8ffa321e40f0c6"
        );
        assert_eq!(
            hex::encode(child_index_with_grandchild.full_hash),
            "80c2b6364721221e7f87028c0482e1e16f49a29889e357c8acab8cb26d4d99da"
        );
        assert_eq!(
            hex::encode(grandchild_index.full_hash),
            "648aa5c579fb30f38af744d97d6ec840c7a91277a499a0d780f3e7314eca090b"
        );

        let greatgrandchild_id = Id::random();
        let greatgrandchild_hash = [4_u8; 32];
        let greatgrandchild_info = ChildInfo::new(
            greatgrandchild_id,
            greatgrandchild_hash,
            Metadata::default(),
        );
        assert!(<Index<MainStorage>>::add_child_to(
            grandchild_id,
            greatgrandchild_collection_name,
            greatgrandchild_info,
        )
        .is_ok());

        let root_index_with_greatgrandchild =
            <Index<MainStorage>>::get_index(root_id).unwrap().unwrap();
        let child_index_with_greatgrandchild =
            <Index<MainStorage>>::get_index(child_id).unwrap().unwrap();
        let grandchild_index_with_greatgrandchild = <Index<MainStorage>>::get_index(grandchild_id)
            .unwrap()
            .unwrap();
        let mut greatgrandchild_index = <Index<MainStorage>>::get_index(greatgrandchild_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            hex::encode(root_index_with_greatgrandchild.full_hash),
            "6bdcb2f1a98eba952d3b2cf43c8bb36eb6a50b853d5b49dea089775e17d67b27"
        );
        assert_eq!(
            hex::encode(child_index_with_greatgrandchild.full_hash),
            "8aca1399f292c2ed8dfaba100a7885c7ac108b7b6b32f10d4a3e9c05fd7c38c0"
        );
        assert_eq!(
            hex::encode(grandchild_index_with_greatgrandchild.full_hash),
            "135605b30fda6d313c472745c4445edb4e8c619cdcc24caa2352c12aacd18a76"
        );
        assert_eq!(
            hex::encode(greatgrandchild_index.full_hash),
            "9f4fb68f3e1dac82202f9aa581ce0bbf1f765df0e9ac3c8c57e20f685abab8ed"
        );

        greatgrandchild_index.own_hash = [9_u8; 32];
        <Index<MainStorage>>::save_index(&greatgrandchild_index).unwrap();
        greatgrandchild_index.full_hash =
            <Index<MainStorage>>::calculate_full_merkle_hash_for(greatgrandchild_id, false)
                .unwrap();
        <Index<MainStorage>>::save_index(&greatgrandchild_index).unwrap();

        <Index<MainStorage>>::recalculate_ancestor_hashes_for(greatgrandchild_id).unwrap();

        let updated_root_index_with_greatgrandchild =
            <Index<MainStorage>>::get_index(root_id).unwrap().unwrap();
        let updated_child_index_with_greatgrandchild =
            <Index<MainStorage>>::get_index(child_id).unwrap().unwrap();
        let updated_grandchild_index_with_greatgrandchild =
            <Index<MainStorage>>::get_index(grandchild_id)
                .unwrap()
                .unwrap();
        let updated_greatgrandchild_index = <Index<MainStorage>>::get_index(greatgrandchild_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            hex::encode(updated_root_index_with_greatgrandchild.full_hash),
            "f61c8077c7875e38a3cbdce3b3d4ce40a5a18add8ce386803760484772bcb85b"
        );
        assert_eq!(
            hex::encode(updated_child_index_with_greatgrandchild.full_hash),
            "abef09c52909317783e0c582553a8fb19124249d93f8878cf131b8dd28fbb4bf"
        );
        assert_eq!(
            hex::encode(updated_grandchild_index_with_greatgrandchild.full_hash),
            "97b2d3a1682881ec11e747f3dd4c242a33f8cff6c6d6224e1dd23278eef35554"
        );
        assert_eq!(
            hex::encode(updated_greatgrandchild_index.full_hash),
            "8c0cc17a04942cc4f8e0fe0b302606d3108860c126428ba2ceeb5f9ed41c2b05"
        );

        greatgrandchild_index.own_hash = [99_u8; 32];
        <Index<MainStorage>>::save_index(&greatgrandchild_index).unwrap();
        greatgrandchild_index.full_hash =
            <Index<MainStorage>>::calculate_full_merkle_hash_for(greatgrandchild_id, false)
                .unwrap();
        <Index<MainStorage>>::save_index(&greatgrandchild_index).unwrap();

        <Index<MainStorage>>::recalculate_ancestor_hashes_for(greatgrandchild_id).unwrap();

        let updated_root_index_with_greatgrandchild =
            <Index<MainStorage>>::get_index(root_id).unwrap().unwrap();
        let updated_child_index_with_greatgrandchild =
            <Index<MainStorage>>::get_index(child_id).unwrap().unwrap();
        let updated_grandchild_index_with_greatgrandchild =
            <Index<MainStorage>>::get_index(grandchild_id)
                .unwrap()
                .unwrap();
        let updated_greatgrandchild_index = <Index<MainStorage>>::get_index(greatgrandchild_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            hex::encode(updated_root_index_with_greatgrandchild.full_hash),
            "0483e0a8a3c3002a94c3ce2e1f7fcadae4b2dc29e2dee9752b9caa683dfe39fc"
        );
        assert_eq!(
            hex::encode(updated_child_index_with_greatgrandchild.full_hash),
            "a7bad731e6767c36725a7c592174fdfe799c6bc32e92cc0f455e6ec5f6e5d42b"
        );
        assert_eq!(
            hex::encode(updated_grandchild_index_with_greatgrandchild.full_hash),
            "67eb9aff17a7db347e4c56264042dcfb1f4e465f70abb56a2108571316435ea5"
        );
        assert_eq!(
            hex::encode(updated_greatgrandchild_index.full_hash),
            "cd93782b7fb95559de14f738b65988af85d41dc1565f7c7d1ed2d035665b519c"
        );
    }

    #[test]
    fn update_hash_for__full() {
        let root_id = Id::random();
        let root_hash0 = [0_u8; 32];
        let root_hash1 = [1_u8; 32];
        let root_hash2 = [2_u8; 32];
        let root_full_hash: [u8; 32] =
            hex::decode("75877bb41d393b5fb8455ce60ecd8dda001d06316496b14dfa7f895656eeca4a")
                .unwrap()
                .try_into()
                .unwrap();

        assert!(<Index<MainStorage>>::add_root(ChildInfo::new(
            root_id,
            root_hash1,
            Metadata::default()
        ),)
        .is_ok());

        let root_index = <Index<MainStorage>>::get_index(root_id).unwrap().unwrap();
        assert_eq!(root_index.id, root_id);
        assert_eq!(root_index.full_hash, root_hash0);

        assert!(<Index<MainStorage>>::update_hash_for(root_id, root_hash2, None).is_ok());
        let updated_root_index = <Index<MainStorage>>::get_index(root_id).unwrap().unwrap();
        assert_eq!(updated_root_index.id, root_id);
        assert_eq!(updated_root_index.full_hash, root_full_hash);
    }

    #[test]
    fn update_hash_for__own() {
        let root_id = Id::random();
        let root_hash1 = [1_u8; 32];
        let root_hash2 = [2_u8; 32];

        assert!(<Index<MainStorage>>::add_root(ChildInfo::new(
            root_id,
            root_hash1,
            Metadata::default()
        ),)
        .is_ok());

        let root_index = <Index<MainStorage>>::get_index(root_id).unwrap().unwrap();
        assert_eq!(root_index.id, root_id);
        assert_eq!(root_index.own_hash, root_hash1);

        assert!(<Index<MainStorage>>::update_hash_for(root_id, root_hash2, None).is_ok());
        let updated_root_index = <Index<MainStorage>>::get_index(root_id).unwrap().unwrap();
        assert_eq!(updated_root_index.id, root_id);
        assert_eq!(updated_root_index.own_hash, root_hash2);
    }
}

use super::*;

#[test]
fn drive_and_unc_roots_have_finite_parent_chains() {
    for root in [
        b"C:/".as_slice(),
        b"//server/share/",
        b"//?/C:/",
        b"//?/UNC/server/share/",
    ] {
        assert!(is_root(root));
        assert_eq!(parent_path(root), Some(root));
        let mut counts = Counts::new(false);
        counts.add(&[root, b"file"].concat());
        counts.add(&[root, b"nested/file"].concat());
        counts.aggregate();
        assert_eq!(counts.nodes[counts.directories[root]].recursive, 2);
    }
}

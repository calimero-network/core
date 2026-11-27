from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.frontend_file_digest import FrontendFileDigest


T = TypeVar("T", bound="FrontendDigests")


@_attrs_define
class FrontendDigests:
    """`GET /packages/{package}/{version}/frontend`: the release's frontend entry and every file, by path relative to the
    frontend root, with its SHA-256 (lowercase hex) and size in bytes.

        Attributes:
            package (str):
            version (str):
            entry (str):
            files (list['FrontendFileDigest']):
    """

    package: str
    version: str
    entry: str
    files: list["FrontendFileDigest"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.frontend_file_digest import FrontendFileDigest

        package = self.package

        version = self.version

        entry = self.entry

        files = []
        for files_item_data in self.files:
            files_item = files_item_data.to_dict()
            files.append(files_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "package": package,
                "version": version,
                "entry": entry,
                "files": files,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.frontend_file_digest import FrontendFileDigest

        d = dict(src_dict)
        package = d.pop("package")

        version = d.pop("version")

        entry = d.pop("entry")

        files = []
        _files = d.pop("files")
        for files_item_data in _files:
            files_item = FrontendFileDigest.from_dict(files_item_data)

            files.append(files_item)

        frontend_digests = cls(
            package=package,
            version=version,
            entry=entry,
            files=files,
        )

        frontend_digests.additional_properties = d
        return frontend_digests

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties

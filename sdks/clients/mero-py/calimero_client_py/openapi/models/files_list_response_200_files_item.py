from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="FilesListResponse200FilesItem")


@_attrs_define
class FilesListResponse200FilesItem:
    """
    Attributes:
        blob_id (str):
        id (str):
        size (int):
        sha256 (str):
    """

    blob_id: str
    id: str
    size: int
    sha256: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        blob_id = self.blob_id

        id = self.id

        size = self.size

        sha256 = self.sha256

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "blobId": blob_id,
                "id": id,
                "size": size,
                "sha256": sha256,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        blob_id = d.pop("blobId")

        id = d.pop("id")

        size = d.pop("size")

        sha256 = d.pop("sha256")

        files_list_response_200_files_item = cls(
            blob_id=blob_id,
            id=id,
            size=size,
            sha256=sha256,
        )

        files_list_response_200_files_item.additional_properties = d
        return files_list_response_200_files_item

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

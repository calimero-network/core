from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.files_list_response_200_files_item import (
        FilesListResponse200FilesItem,
    )


T = TypeVar("T", bound="FilesListResponse200")


@_attrs_define
class FilesListResponse200:
    """
    Attributes:
        files (list['FilesListResponse200FilesItem']):
    """

    files: list["FilesListResponse200FilesItem"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.files_list_response_200_files_item import (
            FilesListResponse200FilesItem,
        )

        files = []
        for files_item_data in self.files:
            files_item = files_item_data.to_dict()
            files.append(files_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "files": files,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.files_list_response_200_files_item import (
            FilesListResponse200FilesItem,
        )

        d = dict(src_dict)
        files = []
        _files = d.pop("files")
        for files_item_data in _files:
            files_item = FilesListResponse200FilesItem.from_dict(files_item_data)

            files.append(files_item)

        files_list_response_200 = cls(
            files=files,
        )

        files_list_response_200.additional_properties = d
        return files_list_response_200

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

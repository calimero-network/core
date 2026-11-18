from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.blob_list_response_200_blobs_item import BlobListResponse200BlobsItem


T = TypeVar("T", bound="BlobListResponse200")


@_attrs_define
class BlobListResponse200:
    """
    Attributes:
        blobs (list['BlobListResponse200BlobsItem']):
    """

    blobs: list["BlobListResponse200BlobsItem"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.blob_list_response_200_blobs_item import (
            BlobListResponse200BlobsItem,
        )

        blobs = []
        for blobs_item_data in self.blobs:
            blobs_item = blobs_item_data.to_dict()
            blobs.append(blobs_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "blobs": blobs,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.blob_list_response_200_blobs_item import (
            BlobListResponse200BlobsItem,
        )

        d = dict(src_dict)
        blobs = []
        _blobs = d.pop("blobs")
        for blobs_item_data in _blobs:
            blobs_item = BlobListResponse200BlobsItem.from_dict(blobs_item_data)

            blobs.append(blobs_item)

        blob_list_response_200 = cls(
            blobs=blobs,
        )

        blob_list_response_200.additional_properties = d
        return blob_list_response_200

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

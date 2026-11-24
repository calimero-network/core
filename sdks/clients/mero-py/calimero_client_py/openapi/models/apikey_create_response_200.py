from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.apikey_create_response_200_then_item import (
        ApikeyCreateResponse200ThenItem,
    )


T = TypeVar("T", bound="ApikeyCreateResponse200")


@_attrs_define
class ApikeyCreateResponse200:
    """
    Attributes:
        key_id (str):
        key (str):
        then (list['ApikeyCreateResponse200ThenItem']):
    """

    key_id: str
    key: str
    then: list["ApikeyCreateResponse200ThenItem"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.apikey_create_response_200_then_item import (
            ApikeyCreateResponse200ThenItem,
        )

        key_id = self.key_id

        key = self.key

        then = []
        for then_item_data in self.then:
            then_item = then_item_data.to_dict()
            then.append(then_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "keyId": key_id,
                "key": key,
                "then": then,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.apikey_create_response_200_then_item import (
            ApikeyCreateResponse200ThenItem,
        )

        d = dict(src_dict)
        key_id = d.pop("keyId")

        key = d.pop("key")

        then = []
        _then = d.pop("then")
        for then_item_data in _then:
            then_item = ApikeyCreateResponse200ThenItem.from_dict(then_item_data)

            then.append(then_item)

        apikey_create_response_200 = cls(
            key_id=key_id,
            key=key,
            then=then,
        )

        apikey_create_response_200.additional_properties = d
        return apikey_create_response_200

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

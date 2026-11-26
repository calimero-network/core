from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.api_key import ApiKey


T = TypeVar("T", bound="ApiKeyList")


@_attrs_define
class ApiKeyList:
    """`GET /auth/apikeys`: the account's keys, never a key itself.

    Attributes:
        keys (list['ApiKey']):
    """

    keys: list["ApiKey"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.api_key import ApiKey

        keys = []
        for keys_item_data in self.keys:
            keys_item = keys_item_data.to_dict()
            keys.append(keys_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "keys": keys,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.api_key import ApiKey

        d = dict(src_dict)
        keys = []
        _keys = d.pop("keys")
        for keys_item_data in _keys:
            keys_item = ApiKey.from_dict(keys_item_data)

            keys.append(keys_item)

        api_key_list = cls(
            keys=keys,
        )

        api_key_list.additional_properties = d
        return api_key_list

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

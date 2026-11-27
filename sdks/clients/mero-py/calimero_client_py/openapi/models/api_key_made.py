from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.api_key_made_then_item import ApiKeyMadeThenItem


T = TypeVar("T", bound="ApiKeyMade")


@_attrs_define
class ApiKeyMade:
    """`POST /auth/apikeys`: the key, answered once, and what the calls made with it answered.

    Attributes:
        key_id (str):
        key (str):
        then (list['ApiKeyMadeThenItem']):
    """

    key_id: str
    key: str
    then: list["ApiKeyMadeThenItem"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.api_key_made_then_item import ApiKeyMadeThenItem

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
        from ..models.api_key_made_then_item import ApiKeyMadeThenItem

        d = dict(src_dict)
        key_id = d.pop("keyId")

        key = d.pop("key")

        then = []
        _then = d.pop("then")
        for then_item_data in _then:
            then_item = ApiKeyMadeThenItem.from_dict(then_item_data)

            then.append(then_item)

        api_key_made = cls(
            key_id=key_id,
            key=key,
            then=then,
        )

        api_key_made.additional_properties = d
        return api_key_made

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

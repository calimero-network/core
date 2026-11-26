from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.alias_list_aliases import AliasListAliases


T = TypeVar("T", bound="AliasList")


@_attrs_define
class AliasList:
    """`GET /auth/aliases`: the account's names for apps.

    Attributes:
        aliases (AliasListAliases):
    """

    aliases: "AliasListAliases"
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.alias_list_aliases import AliasListAliases

        aliases = self.aliases.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "aliases": aliases,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.alias_list_aliases import AliasListAliases

        d = dict(src_dict)
        aliases = AliasListAliases.from_dict(d.pop("aliases"))

        alias_list = cls(
            aliases=aliases,
        )

        alias_list.additional_properties = d
        return alias_list

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

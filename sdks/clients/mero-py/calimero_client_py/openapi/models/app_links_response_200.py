from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.app_links_response_200_links_item import AppLinksResponse200LinksItem


T = TypeVar("T", bound="AppLinksResponse200")


@_attrs_define
class AppLinksResponse200:
    """
    Attributes:
        app (str):
        links (list['AppLinksResponse200LinksItem']):
    """

    app: str
    links: list["AppLinksResponse200LinksItem"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.app_links_response_200_links_item import (
            AppLinksResponse200LinksItem,
        )

        app = self.app

        links = []
        for links_item_data in self.links:
            links_item = links_item_data.to_dict()
            links.append(links_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "app": app,
                "links": links,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.app_links_response_200_links_item import (
            AppLinksResponse200LinksItem,
        )

        d = dict(src_dict)
        app = d.pop("app")

        links = []
        _links = d.pop("links")
        for links_item_data in _links:
            links_item = AppLinksResponse200LinksItem.from_dict(links_item_data)

            links.append(links_item)

        app_links_response_200 = cls(
            app=app,
            links=links,
        )

        app_links_response_200.additional_properties = d
        return app_links_response_200

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

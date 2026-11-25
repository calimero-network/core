from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.link_row import LinkRow


T = TypeVar("T", bound="AppLinkList")


@_attrs_define
class AppLinkList:
    """`GET /apps/{app}/links`: each reference of the release the instance runs and what is linked.

    Attributes:
        app (str):
        links (list['LinkRow']):
    """

    app: str
    links: list["LinkRow"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.link_row import LinkRow

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
        from ..models.link_row import LinkRow

        d = dict(src_dict)
        app = d.pop("app")

        links = []
        _links = d.pop("links")
        for links_item_data in _links:
            links_item = LinkRow.from_dict(links_item_data)

            links.append(links_item)

        app_link_list = cls(
            app=app,
            links=links,
        )

        app_link_list.additional_properties = d
        return app_link_list

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

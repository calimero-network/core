from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.consent_page_create_body_type_18_kind import (
    ConsentPageCreateBodyType18Kind,
)
from typing import cast

if TYPE_CHECKING:
    from ..models.consent_page_create_body_type_18_subject import (
        ConsentPageCreateBodyType18Subject,
    )


T = TypeVar("T", bound="ConsentPageCreateBodyType18")


@_attrs_define
class ConsentPageCreateBodyType18:
    """
    Attributes:
        kind (ConsentPageCreateBodyType18Kind):
        subject (ConsentPageCreateBodyType18Subject):
        return_to (str):
    """

    kind: ConsentPageCreateBodyType18Kind
    subject: "ConsentPageCreateBodyType18Subject"
    return_to: str

    def to_dict(self) -> dict[str, Any]:
        from ..models.consent_page_create_body_type_18_subject import (
            ConsentPageCreateBodyType18Subject,
        )

        kind = self.kind.value

        subject = self.subject.to_dict()

        return_to = self.return_to

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "kind": kind,
                "subject": subject,
                "returnTo": return_to,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.consent_page_create_body_type_18_subject import (
            ConsentPageCreateBodyType18Subject,
        )

        d = dict(src_dict)
        kind = ConsentPageCreateBodyType18Kind(d.pop("kind"))

        subject = ConsentPageCreateBodyType18Subject.from_dict(d.pop("subject"))

        return_to = d.pop("returnTo")

        consent_page_create_body_type_18 = cls(
            kind=kind,
            subject=subject,
            return_to=return_to,
        )

        return consent_page_create_body_type_18

from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.consent_page_create_body_type_4_kind import ConsentPageCreateBodyType4Kind
from ..models.consent_page_create_body_type_4_scope_type_0 import (
    ConsentPageCreateBodyType4ScopeType0,
)
from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.consent_page_create_body_type_4_scope_type_1 import (
        ConsentPageCreateBodyType4ScopeType1,
    )
    from ..models.consent_page_create_body_type_4_subject import (
        ConsentPageCreateBodyType4Subject,
    )


T = TypeVar("T", bound="ConsentPageCreateBodyType4")


@_attrs_define
class ConsentPageCreateBodyType4:
    """
    Attributes:
        kind (ConsentPageCreateBodyType4Kind):
        scope (Union['ConsentPageCreateBodyType4ScopeType1', ConsentPageCreateBodyType4ScopeType0]):
        subject (ConsentPageCreateBodyType4Subject):
        return_to (str):
    """

    kind: ConsentPageCreateBodyType4Kind
    scope: Union[
        "ConsentPageCreateBodyType4ScopeType1", ConsentPageCreateBodyType4ScopeType0
    ]
    subject: "ConsentPageCreateBodyType4Subject"
    return_to: str

    def to_dict(self) -> dict[str, Any]:
        from ..models.consent_page_create_body_type_4_scope_type_1 import (
            ConsentPageCreateBodyType4ScopeType1,
        )
        from ..models.consent_page_create_body_type_4_subject import (
            ConsentPageCreateBodyType4Subject,
        )

        kind = self.kind.value

        scope: Union[dict[str, Any], str]
        if isinstance(self.scope, ConsentPageCreateBodyType4ScopeType0):
            scope = self.scope.value
        else:
            scope = self.scope.to_dict()

        subject = self.subject.to_dict()

        return_to = self.return_to

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "kind": kind,
                "scope": scope,
                "subject": subject,
                "returnTo": return_to,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.consent_page_create_body_type_4_scope_type_1 import (
            ConsentPageCreateBodyType4ScopeType1,
        )
        from ..models.consent_page_create_body_type_4_subject import (
            ConsentPageCreateBodyType4Subject,
        )

        d = dict(src_dict)
        kind = ConsentPageCreateBodyType4Kind(d.pop("kind"))

        def _parse_scope(
            data: object,
        ) -> Union[
            "ConsentPageCreateBodyType4ScopeType1", ConsentPageCreateBodyType4ScopeType0
        ]:
            try:
                if not isinstance(data, str):
                    raise TypeError()
                scope_type_0 = ConsentPageCreateBodyType4ScopeType0(data)

                return scope_type_0
            except:  # noqa: E722
                pass
            if not isinstance(data, dict):
                raise TypeError()
            scope_type_1 = ConsentPageCreateBodyType4ScopeType1.from_dict(data)

            return scope_type_1

        scope = _parse_scope(d.pop("scope"))

        subject = ConsentPageCreateBodyType4Subject.from_dict(d.pop("subject"))

        return_to = d.pop("returnTo")

        consent_page_create_body_type_4 = cls(
            kind=kind,
            scope=scope,
            subject=subject,
            return_to=return_to,
        )

        return consent_page_create_body_type_4
